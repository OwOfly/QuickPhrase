//! Snippet retrieval (M1-2): blob match, D16 scoring and ordering.
//!
//! Score: title exact 400, title prefix 300, title contains 200,
//! body contains 100, title-pinyin blob-only 90, tag-only 85 (no highlight).
//! Ties break by pinned, lastUsedAt, updatedAt, sortOrder, id. At most
//! `limit` hits are returned.

use std::time::Instant;

pub const DEFAULT_LIMIT: usize = 50;

pub struct Doc<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub blob: &'a str,
    pub pinned: bool,
    pub last_used_at: Option<&'a str>,
    pub updated_at: &'a str,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Recent,
    MostUsed,
}

impl SortMode {
    pub fn from_setting(value: &str) -> Self {
        if value == "most_used" {
            Self::MostUsed
        } else {
            Self::Recent
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub score: u32,
    pub title_highlight: Option<(usize, usize)>,
    pub content_highlight: Option<(usize, usize)>,
}

/// Splits leading `#tag` tokens from a query. Tags are lowercased; the
/// returned text is the remaining keyword query. Stops at the first token that
/// is not `#`-prefixed, so a literal `#` inside text stays searchable.
pub fn split_tag_query(query: &str) -> (Vec<String>, &str) {
    let mut rest = query.trim();
    let mut tags = Vec::new();
    loop {
        let Some(after) = rest.strip_prefix('#') else {
            break;
        };
        let after = after.trim_start();
        let end = after
            .find(|ch: char| ch.is_whitespace() || ch == '#')
            .unwrap_or(after.len());
        let tag = after[..end].trim().to_lowercase();
        if tag.is_empty() {
            return (tags, "");
        }
        tags.push(tag);
        rest = after[end..].trim_start();
    }
    (tags, rest)
}

pub fn rank(docs: &[Doc<'_>], query: &str, limit: usize) -> Vec<Hit> {
    let allow_blob_fallback = !query.contains(['\n', '\r']);
    let needle = query.trim().to_lowercase();
    let mut scored: Vec<(u32, usize)> = Vec::new();

    for (index, doc) in docs.iter().enumerate() {
        if needle.is_empty() {
            scored.push((0, index));
            continue;
        }
        if !doc.blob.contains(&needle) {
            continue;
        }
        // The blob joins title and content with a newline. A match spanning that
        // boundary must not count: require a real hit in one of the two fields.
        let title_lower = doc.title.to_lowercase();
        let score = if title_lower == needle {
            400
        } else if title_lower.starts_with(&needle) {
            300
        } else if title_lower.contains(&needle) {
            200
        } else if doc.content.to_lowercase().contains(&needle) {
            100
        } else if allow_blob_fallback && needle.chars().count() >= 2 {
            // Blob-only matches come from pinyin keys or tags. Rank them below
            // literal title/content hits and skip highlighting. A single
            // character would match too many initials/tags.
            let pinyin_hit = allow_blob_fallback
                && needle
                    .chars()
                    .all(|ch| ch.is_ascii_alphabetic() || ch == ' ')
                && {
                    let mut tail = doc.blob.rsplit('\n');
                    let initials = tail.next().unwrap_or("");
                    let compact = tail.next().unwrap_or("");
                    let full = tail.next().unwrap_or("");
                    full.contains(&needle)
                        || compact.contains(&needle)
                        || initials.contains(&needle)
                };
            if pinyin_hit {
                90
            } else {
                85
            }
        } else {
            continue;
        };
        scored.push((score, index));
    }

    scored.sort_by(|left, right| {
        let a = &docs[left.1];
        let b = &docs[right.1];
        right
            .0
            .cmp(&left.0)
            .then_with(|| b.pinned.cmp(&a.pinned))
            .then_with(|| cmp_last_used(a.last_used_at, b.last_used_at))
            .then_with(|| b.updated_at.cmp(a.updated_at))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
            .then_with(|| a.id.cmp(b.id))
    });

    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(score, index)| {
            let doc = &docs[index];
            let (title_highlight, content_highlight) = if needle.is_empty() {
                (None, None)
            } else {
                (
                    find_range(doc.title, &needle),
                    find_range(doc.content, &needle),
                )
            };
            Hit {
                index,
                score,
                title_highlight,
                content_highlight,
            }
        })
        .collect()
}

/// Larger timestamps sort first; `None` sorts last.
fn cmp_last_used(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(a), Some(b)) => b.cmp(a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Case-insensitive substring range mapped back to byte offsets in `haystack`.
/// Lowercasing can expand one character into several (`İ` -> `i` + combining
/// dot). The lowered text comes from `str::to_lowercase` (same rules as the
/// blob), while each lowered character keeps a link to the original character,
/// so the returned range always covers whole original characters.
fn find_range(haystack: &str, needle_lower: &str) -> Option<(usize, usize)> {
    if needle_lower.is_empty() {
        return None;
    }

    let lower = haystack.to_lowercase();
    let mut origin: Vec<(usize, usize)> = Vec::with_capacity(lower.len());
    for (offset, character) in haystack.char_indices() {
        let start = offset;
        let end = offset + character.len_utf8();
        for _ in character.to_lowercase() {
            origin.push((start, end));
        }
    }

    let position = lower.find(needle_lower)?;
    let char_start = lower[..position].chars().count();
    let char_len = needle_lower.chars().count();
    let (start, _) = *origin.get(char_start)?;
    let (_, end) = *origin.get(char_start + char_len - 1)?;
    Some((start, end))
}

/// 1,000 documents, 100 queries; returns (p50, p95, max) in microseconds.
pub fn bench(documents: usize, queries: usize) -> (u128, u128, u128) {
    if queries == 0 {
        return (0, 0, 0);
    }
    let titles: Vec<String> = (0..documents)
        .map(|index| format!("项目更新说明 {index} alpha"))
        .collect();
    let contents: Vec<String> = (0..documents)
        .map(|index| format!("这是第 {index} 条正文，包含接口、测试与发布计划。"))
        .collect();
    let blobs: Vec<String> = titles
        .iter()
        .zip(&contents)
        .map(|(title, content)| crate::storage::build_search_blob(title, content, &[]))
        .collect();

    let patterns = [
        "alpha",
        "项目",
        "接口",
        "不存在",
        "说明 9",
        "发布计划",
        "测试",
        "beta",
        "更新",
        "第 1",
        "xiangmu",
        "xm",
    ];
    let mut samples = Vec::with_capacity(queries);
    for index in 0..queries {
        let query = patterns[index % patterns.len()];
        let started = Instant::now();
        // The panel builds this view on every keystroke, so include it.
        let docs: Vec<Doc<'_>> = (0..documents)
            .map(|index| Doc {
                id: "bench",
                title: &titles[index],
                content: &contents[index],
                blob: &blobs[index],
                pinned: index % 7 == 0,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: index as i32,
            })
            .collect();
        let hits = rank(&docs, query, DEFAULT_LIMIT);
        let elapsed = started.elapsed().as_micros();
        std::hint::black_box(hits);
        samples.push(elapsed);
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    // Nearest-rank p95: ceil(0.95 * n) -> zero-based index.
    let p95_rank = (samples.len() * 95).div_ceil(100);
    let p95 = samples[p95_rank.saturating_sub(1).min(samples.len() - 1)];
    let max = *samples.last().unwrap_or(&0);
    (p50, p95, max)
}

#[cfg(test)]
mod tests {
    use super::{bench, rank, split_tag_query, Doc};
    use crate::storage::build_search_blob;

    fn doc<'a>(
        id: &'a str,
        title: &'a str,
        content: &'a str,
        pinned: bool,
        last_used_at: Option<&'a str>,
        updated_at: &'a str,
        sort_order: i32,
    ) -> Doc<'a> {
        Doc {
            id,
            title,
            content,
            blob: "",
            pinned,
            last_used_at,
            updated_at,
            sort_order,
        }
    }

    fn corpus() -> Vec<Doc<'static>> {
        vec![
            doc(
                "1",
                "项目进度回复",
                "本周完成接口联调。",
                false,
                None,
                "2026-01-02T00:00:00Z",
                0,
            ),
            doc(
                "2",
                "项目",
                "独立的标题精确匹配。",
                false,
                None,
                "2026-01-01T00:00:00Z",
                1,
            ),
            doc(
                "3",
                "收到回复",
                "好的，已收到。",
                false,
                None,
                "2026-01-03T00:00:00Z",
                2,
            ),
            doc(
                "4",
                "接口说明",
                "返回 JSON。",
                true,
                Some("2026-02-01T00:00:00Z"),
                "2026-01-01T00:00:00Z",
                3,
            ),
        ]
    }

    #[test]
    fn empty_query_returns_all_in_d16_order() {
        let docs = corpus();
        let hits = rank(&docs, "", 50);
        let order: Vec<usize> = hits.iter().map(|hit| hit.index).collect();
        // pinned 3, then updatedAt desc: 2 (01-03), 0 (01-02), 1 (01-01).
        assert_eq!(order, vec![3, 2, 0, 1]);
    }

    #[test]
    fn scoring_orders_exact_prefix_contains_body() {
        let titles = ["项目", "项目进度", "本周项目总结", "收到回复"];
        let contents = ["无", "无", "无", "项目相关内容"];
        let blobs: Vec<String> = titles
            .iter()
            .zip(contents)
            .map(|(title, content)| format!("{}\n{}", title.to_lowercase(), content.to_lowercase()))
            .collect();
        let docs: Vec<Doc> = (0..4)
            .map(|index| Doc {
                id: "x",
                title: titles[index],
                content: contents[index],
                blob: &blobs[index],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: index as i32,
            })
            .collect();
        let hits = rank(&docs, "项目", 50);
        let scores: Vec<u32> = hits.iter().map(|hit| hit.score).collect();
        assert_eq!(scores, vec![400, 300, 200, 100]);
    }

    #[test]
    fn case_insensitive_and_highlight_ranges() {
        let titles = ["JSON 接口"];
        let contents = ["返回 Json 数据"];
        let blobs = vec!["json 接口\n返回 json 数据".to_string()];
        let docs = vec![Doc {
            id: "1",
            title: titles[0],
            content: contents[0],
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        let hits = rank(&docs, "json", 50);
        assert_eq!(hits.len(), 1);
        let (start, end) = hits[0].title_highlight.unwrap();
        assert_eq!(&titles[0][start..end].to_lowercase(), "json");
        let (start, end) = hits[0].content_highlight.unwrap();
        assert_eq!(&contents[0][start..end].to_lowercase(), "json");
    }

    #[test]
    fn ties_break_by_last_used_then_updated() {
        let titles = ["同名", "同名", "同名"];
        let contents = ["a", "b", "c"];
        let blobs: Vec<String> = (0..3)
            .map(|index| format!("同名\n{}", contents[index]))
            .collect();
        let docs: Vec<Doc> = vec![
            Doc {
                id: "a",
                title: titles[0],
                content: contents[0],
                blob: &blobs[0],
                pinned: false,
                last_used_at: Some("2026-02-01T00:00:00Z"),
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 2,
            },
            Doc {
                id: "b",
                title: titles[1],
                content: contents[1],
                blob: &blobs[1],
                pinned: false,
                last_used_at: Some("2026-03-01T00:00:00Z"),
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 1,
            },
            Doc {
                id: "c",
                title: titles[2],
                content: contents[2],
                blob: &blobs[2],
                pinned: false,
                last_used_at: Some("2026-03-01T00:00:00Z"),
                updated_at: "2026-04-01T00:00:00Z",
                sort_order: 0,
            },
        ];
        let hits = rank(&docs, "同名", 50);
        let ids: Vec<&str> = hits.iter().map(|hit| docs[hit.index].id).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn limit_is_applied_after_ordering() {
        // Input order is the reverse of sortOrder, so pre-sort truncation would
        // keep a different set than post-sort truncation.
        let titles: Vec<String> = (0..80).map(|index| format!("条目 {index}")).collect();
        let blobs: Vec<String> = titles.iter().map(|title| title.to_lowercase()).collect();
        let docs: Vec<Doc> = (0..80)
            .map(|index| Doc {
                id: "x",
                title: &titles[index],
                content: "",
                blob: &blobs[index],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 79 - index as i32,
            })
            .collect();
        let hits = rank(&docs, "条目", 50);
        assert_eq!(hits.len(), 50);
        assert_eq!(hits[0].index, 79);
        assert_eq!(hits[49].index, 30);
    }

    #[test]
    fn empty_last_used_sorts_last() {
        let titles = ["同名", "同名"];
        let blobs = vec!["同名".to_string(), "同名".to_string()];
        let docs: Vec<Doc> = vec![
            Doc {
                id: "never",
                title: titles[0],
                content: "",
                blob: &blobs[0],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-09-01T00:00:00Z",
                sort_order: 0,
            },
            Doc {
                id: "used",
                title: titles[1],
                content: "",
                blob: &blobs[1],
                pinned: false,
                last_used_at: Some("2026-01-01T00:00:00Z"),
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 1,
            },
        ];
        let hits = rank(&docs, "同名", 50);
        assert_eq!(docs[hits[0].index].id, "used");
    }

    #[test]
    fn pinyin_match_ranks_below_literal_content() {
        let pinyin_title = "退款说明";
        let literal_title = "其他";
        let pinyin_blob = build_search_blob(pinyin_title, "", &[]);
        let literal_blob = build_search_blob(literal_title, "tk", &[]);
        let docs = vec![
            Doc {
                id: "pinyin",
                title: pinyin_title,
                content: "",
                blob: &pinyin_blob,
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 0,
            },
            Doc {
                id: "literal",
                title: literal_title,
                content: "tk",
                blob: &literal_blob,
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 1,
            },
        ];
        let hits = rank(&docs, "tk", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].index, 1, "literal content must beat pinyin");
        assert_eq!(hits[1].index, 0);
        assert!(hits[1].title_highlight.is_none());
        assert!(hits[1].content_highlight.is_none());
    }

    #[test]
    fn newline_spanning_blob_boundary_does_not_match() {
        let title = "说明";
        let content = "返回";
        let blob = "说明\n返回".to_string();
        let docs = vec![Doc {
            id: "1",
            title,
            content,
            blob: &blob,
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        assert!(rank(&docs, "说明\n返回", 50).is_empty());
    }

    #[test]
    fn empty_title_matches_body_without_title_highlight() {
        let blobs = vec!["\n返回格式为 json".to_string()];
        let docs = vec![Doc {
            id: "1",
            title: "",
            content: "返回格式为 JSON",
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        let hits = rank(&docs, "json", 50);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 100);
        assert!(hits[0].title_highlight.is_none());
        assert!(hits[0].content_highlight.is_some());
    }

    #[test]
    fn unmatched_query_returns_nothing() {
        let docs = corpus();
        assert!(rank(&docs, "完全不存在", 50).is_empty());
    }

    #[test]
    fn query_is_trimmed() {
        let blobs = vec!["项目".to_string()];
        let docs = vec![Doc {
            id: "1",
            title: "项目",
            content: "",
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        assert_eq!(rank(&docs, " 项目 ", 50).len(), 1);
    }

    #[test]
    fn cjk_highlight_covers_whole_characters() {
        let titles = ["项目进度"];
        let blobs = vec!["项目进度".to_string()];
        let docs = vec![Doc {
            id: "1",
            title: titles[0],
            content: "",
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        let hits = rank(&docs, "进度", 50);
        let (start, end) = hits[0].title_highlight.unwrap();
        assert_eq!(&titles[0][start..end], "进度");
    }

    #[test]
    fn expanding_lowercase_maps_back_to_the_original_character() {
        // U+0130 lowercases to "i" plus a combining dot.
        let title = "İab";
        let blobs = vec!["i\u{307}ab".to_string()];
        let docs = vec![Doc {
            id: "1",
            title,
            content: "",
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        let hits = rank(&docs, "a", 50);
        assert_eq!(hits.len(), 1, "query must match through the lowered blob");
        let (start, end) = hits[0].title_highlight.unwrap();
        assert_eq!(&title[start..end], "a");
    }

    #[test]
    fn final_sigma_matches_through_whole_string_lowercase() {
        let title = "λόγος";
        let blobs = vec![title.to_lowercase()];
        let docs = vec![Doc {
            id: "1",
            title,
            content: "",
            blob: &blobs[0],
            pinned: false,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        let hits = rank(&docs, title, 50);
        assert_eq!(hits.len(), 1);
        let (start, end) = hits[0].title_highlight.expect("final sigma highlight");
        assert_eq!(&title[start..end], title);
    }

    #[test]
    fn sort_order_then_id_breaks_ties() {
        let titles = ["同名", "同名", "同名", "同名"];
        let blobs: Vec<String> = titles.iter().map(|title| title.to_string()).collect();
        let docs: Vec<Doc> = vec![
            Doc {
                id: "b",
                title: titles[0],
                content: "",
                blob: &blobs[0],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 1,
            },
            Doc {
                id: "c",
                title: titles[1],
                content: "",
                blob: &blobs[1],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 0,
            },
            Doc {
                id: "a",
                title: titles[2],
                content: "",
                blob: &blobs[2],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 0,
            },
            Doc {
                id: "d",
                title: titles[3],
                content: "",
                blob: &blobs[3],
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 0,
            },
        ];
        let hits = rank(&docs, "同名", 50);
        let ids: Vec<&str> = hits.iter().map(|hit| docs[hit.index].id).collect();
        assert_eq!(ids, vec!["a", "c", "d", "b"]);
    }

    #[test]
    fn splits_leading_tag_tokens() {
        assert_eq!(
            split_tag_query("#Support #refund 退款"),
            (vec!["support".to_string(), "refund".to_string()], "退款")
        );
        assert_eq!(
            split_tag_query("#a#b"),
            (vec!["a".to_string(), "b".to_string()], "")
        );
        assert_eq!(
            split_tag_query("# support"),
            (vec!["support".to_string()], "")
        );
        assert_eq!(split_tag_query("普通 #tag"), (Vec::new(), "普通 #tag"));
        assert_eq!(split_tag_query("#"), (Vec::new(), ""));
    }

    #[test]
    fn bench_runs_at_1000_documents() {
        let (_, p95, _) = bench(1_000, 20);
        assert!(p95 < 50_000, "p95={p95}us");
    }
}
