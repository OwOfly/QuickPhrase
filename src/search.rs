//! Snippet retrieval (M1-2): blob match, D16 scoring and ordering.
//!
//! Score: title exact 400, title prefix 300, title contains 200,
//! body contains 100, title-pinyin blob-only 90, tag-only 85 (no highlight).
//! Ties break by pinned, useCount (MostUsed only), lastUsedAt, updatedAt, sortOrder, id. At most
//! `limit` hits are returned.

use std::borrow::Cow;
use std::time::Instant;

pub const DEFAULT_LIMIT: usize = 50;

/// Lowercased title/content prepared once per panel snapshot.
pub struct LowerKeys {
    pub title: String,
    pub content: String,
}

/// Borrowed searchable fields and deterministic ranking tie-breakers.
pub struct Doc<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub blob: &'a str,
    pub pinned: bool,
    pub use_count: u64,
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
/// is not `#`-prefixed. A leading `\#` escapes tag syntax; a bare `#`
/// and hashes within plain text stay searchable.
pub fn split_tag_query(query: &str) -> (Vec<String>, &str) {
    let mut rest = query.trim();
    let mut tags = Vec::new();
    loop {
        if rest.starts_with(r"\#") {
            return (tags, &rest[1..]);
        }
        let Some(after) = rest.strip_prefix('#') else {
            break;
        };
        let after = after.trim_start();
        let end = after
            .find(|ch: char| ch.is_whitespace() || ch == '#')
            .unwrap_or(after.len());
        let tag = after[..end].trim().to_lowercase();
        if tag.is_empty() {
            break;
        }
        tags.push(tag);
        rest = after[end..].trim_start();
    }
    (tags, rest)
}

/// Rank by relevance and recency.
pub fn rank(docs: &[Doc<'_>], query: &str, limit: usize) -> Vec<Hit> {
    rank_with_mode(docs, query, limit, SortMode::Recent)
}

/// Rank without building highlight ranges; returns indices into `docs`.
pub fn rank_indices(docs: &[Doc<'_>], query: &str, limit: usize) -> Vec<usize> {
    rank_indices_with_mode(docs, query, limit, SortMode::Recent)
}

/// Rank without highlights, in the same order as [`rank_with_mode`].
pub fn rank_indices_with_mode(
    docs: &[Doc<'_>],
    query: &str,
    limit: usize,
    mode: SortMode,
) -> Vec<usize> {
    let (_, mut scored) = rank_scored(docs, query, mode);
    scored.truncate(limit);
    scored.into_iter().map(|(_, index)| index).collect()
}

/// Rank by relevance and pinning first, comparing usage before recency only
/// in MostUsed mode. Compute highlights only after limiting the result set.
pub fn rank_with_mode(docs: &[Doc<'_>], query: &str, limit: usize, mode: SortMode) -> Vec<Hit> {
    let (needle, scored) = rank_scored(docs, query, mode);
    build_hits(docs, &needle, scored, limit)
}

/// Rank using title/content lowercased once by the caller. The keys must be
/// aligned with `docs`; a missing entry falls back to lowercasing that doc.
pub fn rank_prepared_with_mode(
    docs: &[Doc<'_>],
    keys: &[&LowerKeys],
    query: &str,
    limit: usize,
    mode: SortMode,
) -> Vec<Hit> {
    let (needle, scored) = rank_scored_with_keys(docs, query, mode, Some(keys));
    build_hits(docs, &needle, scored, limit)
}

fn build_hits(
    docs: &[Doc<'_>],
    needle: &str,
    mut scored: Vec<(u32, usize)>,
    limit: usize,
) -> Vec<Hit> {
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(score, index)| {
            let doc = &docs[index];
            let (title_highlight, content_highlight) = if needle.is_empty() {
                (None, None)
            } else {
                (
                    find_range(doc.title, needle),
                    find_range(doc.content, needle),
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

/// Shared matching and ordering for both the highlight and index-only APIs.
fn rank_scored(docs: &[Doc<'_>], query: &str, mode: SortMode) -> (String, Vec<(u32, usize)>) {
    rank_scored_with_keys(docs, query, mode, None)
}

fn rank_scored_with_keys(
    docs: &[Doc<'_>],
    query: &str,
    mode: SortMode,
    keys: Option<&[&LowerKeys]>,
) -> (String, Vec<(u32, usize)>) {
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
        let prepared = keys.and_then(|keys| keys.get(index));
        let title_lower: Cow<'_, str> = match prepared {
            Some(key) => Cow::Borrowed(key.title.as_str()),
            None => Cow::Owned(doc.title.to_lowercase()),
        };
        let title_lower = title_lower.as_ref();
        let score = if title_lower == needle.as_str() {
            400
        } else if title_lower.starts_with(needle.as_str()) {
            300
        } else if title_lower.contains(needle.as_str()) {
            200
        } else {
            // Lowercase the body only when the title did not already match.
            let content_lower: Cow<'_, str> = match prepared {
                Some(key) => Cow::Borrowed(key.content.as_str()),
                None => Cow::Owned(doc.content.to_lowercase()),
            };
            if content_lower.contains(needle.as_str()) {
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
            }
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
            .then_with(|| match mode {
                SortMode::Recent => std::cmp::Ordering::Equal,
                SortMode::MostUsed => b.use_count.cmp(&a.use_count),
            })
            .then_with(|| cmp_last_used(a.last_used_at, b.last_used_at))
            .then_with(|| b.updated_at.cmp(a.updated_at))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
            .then_with(|| a.id.cmp(b.id))
    });

    (needle, scored)
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

/// Benchmarks search over the requested corpus (`--search-bench --large` uses
/// 10,000 documents x 1,000 queries); returns (p50, p95, max) in microseconds.
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
                use_count: 0,
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
    use super::{
        bench, rank, rank_indices_with_mode, rank_prepared_with_mode, rank_with_mode,
        split_tag_query, Doc, LowerKeys, SortMode,
    };
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
            use_count: 0,
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
                use_count: 0,
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
        let blobs = ["json 接口\n返回 json 数据".to_string()];
        let docs = [Doc {
            id: "1",
            title: titles[0],
            content: contents[0],
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
        let blobs = ["同名".to_string(), "同名".to_string()];
        let docs: Vec<Doc> = vec![
            Doc {
                id: "never",
                title: titles[0],
                content: "",
                blob: &blobs[0],
                pinned: false,
                use_count: 0,
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
                use_count: 0,
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
        let docs = [
            Doc {
                id: "pinyin",
                title: pinyin_title,
                content: "",
                blob: &pinyin_blob,
                pinned: false,
                use_count: 0,
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
                use_count: 0,
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
        let docs = [Doc {
            id: "1",
            title,
            content,
            blob: &blob,
            pinned: false,
            use_count: 0,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        assert!(rank(&docs, "说明\n返回", 50).is_empty());
    }

    #[test]
    fn empty_title_matches_body_without_title_highlight() {
        let blobs = ["\n返回格式为 json".to_string()];
        let docs = [Doc {
            id: "1",
            title: "",
            content: "返回格式为 JSON",
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
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
        let blobs = ["项目".to_string()];
        let docs = [Doc {
            id: "1",
            title: "项目",
            content: "",
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z",
            sort_order: 0,
        }];
        assert_eq!(rank(&docs, " 项目 ", 50).len(), 1);
    }

    #[test]
    fn cjk_highlight_covers_whole_characters() {
        let titles = ["项目进度"];
        let blobs = ["项目进度".to_string()];
        let docs = [Doc {
            id: "1",
            title: titles[0],
            content: "",
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
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
        let blobs = ["i\u{307}ab".to_string()];
        let docs = [Doc {
            id: "1",
            title,
            content: "",
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
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
        let blobs = [title.to_lowercase()];
        let docs = [Doc {
            id: "1",
            title,
            content: "",
            blob: &blobs[0],
            pinned: false,
            use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
                use_count: 0,
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
        assert_eq!(split_tag_query("#"), (Vec::new(), "#"));
        assert_eq!(split_tag_query(r"\#include"), (Vec::new(), "#include"));
        assert_eq!(
            split_tag_query(r"#dev #cpp \#include <stdio.h>"),
            (
                vec!["dev".to_string(), "cpp".to_string()],
                "#include <stdio.h>"
            )
        );
    }

    #[test]
    fn bench_runs_at_1000_documents() {
        let (_, p95, _) = bench(1_000, 20);
        assert!(p95 < 50_000, "p95={p95}us");
    }

    #[test]
    fn index_only_rank_matches_highlight_rank() {
        let titles = ["项目进度", "接口说明", "收到回复", "API"];
        let contents = ["本周完成接口联调", "返回 JSON", "已收到", "api 文档"];
        let blobs: Vec<String> = titles
            .iter()
            .zip(contents)
            .map(|(title, content)| build_search_blob(title, content, &["support".to_string()]))
            .collect();
        let docs: Vec<Doc> = (0..titles.len())
            .map(|index| Doc {
                id: titles[index],
                title: titles[index],
                content: contents[index],
                blob: &blobs[index],
                pinned: index == 3,
                use_count: index as u64,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: index as i32,
            })
            .collect();
        for query in ["", "项目", "api", "接口", "support", "xiangmu"] {
            for mode in [SortMode::Recent, SortMode::MostUsed] {
                for limit in [0, 1, 2, 50] {
                    let hits = rank_with_mode(&docs, query, limit, mode);
                    let indices = rank_indices_with_mode(&docs, query, limit, mode);
                    let expected: Vec<usize> = hits.iter().map(|hit| hit.index).collect();
                    assert_eq!(
                        indices, expected,
                        "query={query:?} mode={mode:?} limit={limit}"
                    );
                }
            }
        }
    }

    #[test]
    fn index_only_rank_truncates_ties_by_id() {
        let title = "同样的标题";
        let content = "同样的正文";
        let blob = build_search_blob(title, content, &[]);
        let docs: Vec<Doc> = ["c", "a", "b"]
            .iter()
            .map(|id| Doc {
                id,
                title,
                content,
                blob: &blob,
                pinned: false,
                use_count: 0,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: 0,
            })
            .collect();
        for mode in [SortMode::Recent, SortMode::MostUsed] {
            let indices = rank_indices_with_mode(&docs, "", 2, mode);
            let hits = rank_with_mode(&docs, "", 2, mode);
            let expected: Vec<usize> = hits.iter().map(|hit| hit.index).collect();
            assert_eq!(indices, expected);
            let ids: Vec<&str> = indices.iter().map(|&index| docs[index].id).collect();
            assert_eq!(ids, vec!["a", "b"]);
        }
    }

    #[test]
    fn prepared_rank_matches_normal_rank() {
        let titles = ["项目进度", "接口说明", "收到回复", "API"];
        let contents = ["本周完成接口联调", "返回 JSON", "已收到", "api 文档"];
        let blobs: Vec<String> = titles
            .iter()
            .zip(contents)
            .map(|(title, content)| build_search_blob(title, content, &["support".to_string()]))
            .collect();
        let docs: Vec<Doc> = (0..titles.len())
            .map(|index| Doc {
                id: titles[index],
                title: titles[index],
                content: contents[index],
                blob: &blobs[index],
                pinned: index == 3,
                use_count: index as u64,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z",
                sort_order: index as i32,
            })
            .collect();
        let keys: Vec<LowerKeys> = titles
            .iter()
            .zip(contents)
            .map(|(title, content)| LowerKeys {
                title: title.to_lowercase(),
                content: content.to_lowercase(),
            })
            .collect();
        let key_refs: Vec<&LowerKeys> = keys.iter().collect();
        for query in ["", "项目", "api", "接口", "support", "xiangmu"] {
            for mode in [SortMode::Recent, SortMode::MostUsed] {
                for limit in [0, 1, 2, 50] {
                    let normal = rank_with_mode(&docs, query, limit, mode);
                    let prepared = rank_prepared_with_mode(&docs, &key_refs, query, limit, mode);
                    let normal: Vec<_> = normal
                        .iter()
                        .map(|hit| {
                            (
                                hit.index,
                                hit.score,
                                hit.title_highlight,
                                hit.content_highlight,
                            )
                        })
                        .collect();
                    let prepared: Vec<_> = prepared
                        .iter()
                        .map(|hit| {
                            (
                                hit.index,
                                hit.score,
                                hit.title_highlight,
                                hit.content_highlight,
                            )
                        })
                        .collect();
                    assert_eq!(
                        prepared, normal,
                        "query={query:?} mode={mode:?} limit={limit}"
                    );
                    let short = &key_refs[..2];
                    let fallback = rank_prepared_with_mode(&docs, short, query, limit, mode);
                    let fallback: Vec<_> = fallback
                        .iter()
                        .map(|hit| {
                            (
                                hit.index,
                                hit.score,
                                hit.title_highlight,
                                hit.content_highlight,
                            )
                        })
                        .collect();
                    assert_eq!(
                        fallback, normal,
                        "short keys query={query:?} mode={mode:?} limit={limit}"
                    );
                }
            }
        }
    }
}
