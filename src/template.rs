//! Insert-time template expansion for `{date}`, `{time}` and `{datetime}`.
//! Unknown tokens and braces are left untouched. `{{` / `}}` escape braces.

use windows::Win32::System::SystemInformation::GetLocalTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub date: String,
    pub time: String,
}

pub fn now() -> Stamp {
    let value = unsafe { GetLocalTime() };
    Stamp {
        date: format!("{:04}-{:02}-{:02}", value.wYear, value.wMonth, value.wDay),
        time: format!("{:02}:{:02}", value.wHour, value.wMinute),
    }
}

pub fn render(text: &str) -> String {
    if !text.contains(['{', '}']) {
        return text.to_string();
    }
    render_with(text, &now())
}

pub fn render_with(text: &str, stamp: &Stamp) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push('{');
                    continue;
                }
                let mut token = String::new();
                let mut closed = false;
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        closed = true;
                        break;
                    }
                    if next == '{' {
                        break;
                    }
                    token.push(next);
                    chars.next();
                }
                if closed {
                    match token.as_str() {
                        "date" => out.push_str(&stamp.date),
                        "time" => out.push_str(&stamp.time),
                        "datetime" => {
                            out.push_str(&stamp.date);
                            out.push(' ');
                            out.push_str(&stamp.time);
                        }
                        _ => {
                            out.push('{');
                            out.push_str(&token);
                            out.push('}');
                        }
                    }
                } else {
                    out.push('{');
                    out.push_str(&token);
                }
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp {
            date: "2026-11-20".to_string(),
            time: "09:05".to_string(),
        }
    }

    #[test]
    fn expands_date_time_and_datetime() {
        assert_eq!(
            render_with("{date} {time} {datetime}", &stamp()),
            "2026-11-20 09:05 2026-11-20 09:05"
        );
    }

    #[test]
    fn unknown_escaped_nested_and_unclosed_tokens() {
        assert_eq!(render_with("{unknown}", &stamp()), "{unknown}");
        assert_eq!(render_with("{DATE}", &stamp()), "{DATE}");
        assert_eq!(render_with("{}", &stamp()), "{}");
        assert_eq!(render_with("{{date}}", &stamp()), "{date}");
        assert_eq!(render_with("x{{y", &stamp()), "x{y");
        assert_eq!(render_with("a}}b", &stamp()), "a}b");
        assert_eq!(render_with("{{{{", &stamp()), "{{");
        assert_eq!(render_with("}}}}", &stamp()), "}}");
        assert_eq!(render_with("{date{time}", &stamp()), "{date09:05");
        assert_eq!(render_with("日期{date}", &stamp()), "日期2026-11-20");
        assert_eq!(render_with("{date", &stamp()), "{date");
        assert_eq!(render_with("{a{b}", &stamp()), "{a{b}");
        assert_eq!(render_with("{date}}}", &stamp()), "2026-11-20}");
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(render_with("普通正文", &stamp()), "普通正文");
        assert_eq!(render("普通正文"), "普通正文");
        assert_eq!(render("a}}b"), "a}b");
    }
}
