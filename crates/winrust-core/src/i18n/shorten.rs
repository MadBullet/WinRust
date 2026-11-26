//! Unicode-safe text helpers shared by every component.
//!
//! The Task Manager used to truncate the executable path with byte slicing:
//!
//! ```ignore
//! format!("{}…{}", &p[..head], &p[p.len() - tail..])
//! ```
//!
//! That panics whenever byte 20 (or the tail boundary) lands inside a
//! multi-byte character — which is *always* the case on a Russian Windows
//! (`C:\Users\Иван\AppData\...`, `C:\Program Files\Общие файлы\...`).  Because
//! release builds use `panic = "abort"`, the panic kills the whole process with
//! no message: exactly the "Task Manager just closes" report.

/// Shorten in the middle, keeping `head` characters at the start and enough at
/// the end to fill `max` characters, separated by an ellipsis.
pub fn middle(s: &str, max_chars: usize, head_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let head = head_chars.min(max_chars.saturating_sub(1));
    let tail = max_chars.saturating_sub(head + 1);
    let mut out = String::with_capacity(head + tail + 1);
    out.extend(&chars[..head]);
    out.push('…');
    if tail > 0 {
        out.extend(&chars[chars.len() - tail..]);
    }
    out
}

/// Shorten from the end only (no ellipsis in the middle), used for one-line
/// descriptions where the start of the text matters most.
pub fn end(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let mut out: String = chars[..max_chars.saturating_sub(1)].iter().collect();
    out.push('…');
    out
}

/// Split a string at the first `width` characters — never in the middle of a
/// UTF-16 surrogate pair or a Rust `char`.
pub fn head(s: &str, width: usize) -> &str {
    match s.char_indices().nth(width) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Line-wrap a long text into at most `width` characters per line, breaking on
/// whitespace when possible.
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Wrap a desktop icon's caption into at most `lines` lines of `width` chars.
///
/// [`wrap`] alone is not enough for an icon: a caption may be longer than the
/// cell, and a single word longer than `width` cannot be broken by the word
/// loop at all, so it stayed one enormous line and spilled out of the icon's
/// selection square - the "текст ярлыка не скрывается" report.  This breaks
/// over-long words, caps the line count and puts an ellipsis on the last line
/// when something was dropped, so the caption always fits its cell.
pub fn wrap_capped(s: &str, width: usize, lines: usize) -> String {
    let width = width.max(4);
    let lines = lines.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let mut word = word.to_string();
        loop {
            // How much room is left on the current line.
            let room = width.saturating_sub(if cur.is_empty() {
                0
            } else {
                cur.chars().count() + 1
            });
            if word.chars().count() <= room {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(&word);
                break;
            }
            if room >= 4 {
                // Fill the rest of this line with as much of the word as fits.
                if !cur.is_empty() {
                    cur.push(' ');
                }
                let head: String = word.chars().take(room).collect();
                cur.push_str(&head);
                word = word.chars().skip(room).collect();
            }
            out.push(std::mem::take(&mut cur));
            if out.len() >= lines {
                break;
            }
        }
        if out.len() >= lines {
            break;
        }
    }
    if !cur.is_empty() && out.len() < lines {
        out.push(cur);
    }
    if out.is_empty() {
        return String::new();
    }
    // Anything left over - more words, or a word still being broken - needs the
    // ellipsis, otherwise a truncated caption reads like a complete one.
    let consumed: usize = out.iter().map(|l| l.chars().count()).sum();
    let truncated = consumed < s.chars().filter(|c| !c.is_whitespace()).count();
    if truncated {
        if let Some(last) = out.last_mut() {
            let keep = width.saturating_sub(1);
            let mut t: String = last.chars().take(keep).collect();
            t.push('…');
            *last = t;
        }
    }
    out.truncate(lines);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyrillic_is_never_split() {
        // Byte 20 of this string is in the middle of 'а'.
        let p = r"C:\Users\Иван\AppData\Roaming\WinRust\winrust-taskmgr.exe";
        let out = middle(p, 40, 12);
        assert!(out.contains('…'));
        assert!(out.chars().count() <= 40);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn emoji_survives() {
        let p = "C:\\Users\\😀😀😀\\file.exe";
        let out = middle(p, 12, 6);
        assert!(out.chars().count() <= 12);
    }

    #[test]
    fn short_strings_are_untouched() {
        assert_eq!(middle("C:\\x", 40, 12), "C:\\x");
        assert_eq!(end("abc", 10), "abc");
    }

    #[test]
    fn head_is_char_safe() {
        assert_eq!(head("Привет", 3), "При");
        assert_eq!(head("abc", 10), "abc");
    }

    #[test]
    fn wrap_keeps_all_words() {
        let lines = wrap("one two three four five", 7);
        assert_eq!(lines, vec!["one two", "three", "four", "five"]);
    }
}
