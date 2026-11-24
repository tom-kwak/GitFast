//! Shortcut labels for the current platform.
//!
//! gf's strings write shortcuts with macOS glyphs (`⌘⇧K`, `⌃⇥`, `⏎`).
//! [`local`] turns them into the platform's own notation where they are
//! drawn: on macOS it returns its input unchanged; elsewhere `⌘⇧K` becomes
//! `Ctrl+Shift+K`. Only the label changes: key handling already maps ⌘ to
//! Ctrl through `Modifiers::command()`.
//!
//! The text helpers in `widgets` (`mono`, `dim`) call it, so feature code
//! keeps writing mac glyphs and never calls this itself.

use std::borrow::Cow;

/// `s` with shortcut glyphs in the platform's notation.
pub fn local(s: Cow<'_, str>) -> Cow<'_, str> {
    if cfg!(target_os = "macos") {
        s
    } else {
        to_pc(s)
    }
}

/// Modifier glyphs, in the order PCs write them (Ctrl+Alt+Shift).
fn modifier(c: char) -> Option<(u8, &'static str)> {
    match c {
        '⌘' | '⌃' => Some((0, "Ctrl")),
        '⌥' => Some((1, "Alt")),
        '⇧' => Some((2, "Shift")),
        _ => None,
    }
}

/// Key glyphs spelled out on PCs.
fn key_name(c: char) -> Option<&'static str> {
    match c {
        '⇥' => Some("Tab"),
        '⏎' => Some("Enter"),
        '⌫' => Some("Backspace"),
        '⌦' => Some("Delete"),
        '⎋' => Some("Esc"),
        _ => None,
    }
}

fn is_glyph(c: char) -> bool {
    modifier(c).is_some() || key_name(c).is_some()
}

/// The PC notation of `s`, whatever platform runs it (tested everywhere).
fn to_pc(s: Cow<'_, str>) -> Cow<'_, str> {
    if !s.chars().any(is_glyph) {
        return s;
    }
    let mut out = String::with_capacity(s.len() + 16);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if modifier(c).is_none() {
            match key_name(c) {
                Some(name) => out.push_str(name),
                None => out.push(c),
            }
            continue;
        }
        // A run of modifiers, then the key they go with (if any).
        let mut mods: [Option<&str>; 3] = [None; 3];
        let mut next = Some(c);
        while let Some((i, name)) = next.and_then(modifier) {
            mods[usize::from(i)] = Some(name);
            next = chars.next_if(|n| modifier(*n).is_some());
        }
        let joined: Vec<&str> = mods.into_iter().flatten().collect();
        out.push_str(&joined.join("+"));
        match chars.peek().copied() {
            // `(⌘)` names the key alone; `⌘,` is a shortcut.
            Some(k) if !k.is_whitespace() && k != ')' => {
                chars.next();
                out.push('+');
                match key_name(k) {
                    Some(name) => out.push_str(name),
                    None => out.extend(k.to_uppercase()),
                }
            }
            _ => {}
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(s: &str) -> String {
        to_pc(Cow::Borrowed(s)).into_owned()
    }

    #[test]
    fn modifiers_join_with_plus_in_pc_order() {
        assert_eq!(pc("⌘O"), "Ctrl+O");
        assert_eq!(pc("⌘⇧Z"), "Ctrl+Shift+Z");
        assert_eq!(pc("⇧⌘Z"), "Ctrl+Shift+Z");
        assert_eq!(pc("⌥⌘K"), "Ctrl+Alt+K");
        assert_eq!(pc("⌃⇥"), "Ctrl+Tab");
        assert_eq!(pc("⌃⇧⇥"), "Ctrl+Shift+Tab");
        assert_eq!(pc("⌘⏎"), "Ctrl+Enter");
        assert_eq!(pc("⌘⌫"), "Ctrl+Backspace");
        assert_eq!(pc("settings (⌘,)"), "settings (Ctrl+,)");
        assert_eq!(pc("⌘⇧[ / ⌘⇧]"), "Ctrl+Shift+[ / Ctrl+Shift+]");
        assert_eq!(pc("⌘⌥⇧[ ⌘⌥⇧]"), "Ctrl+Alt+Shift+[ Ctrl+Alt+Shift+]");
    }

    #[test]
    fn keys_are_spelled_out_and_text_is_kept() {
        assert_eq!(pc("⌘1-9"), "Ctrl+1-9");
        assert_eq!(pc("⌘k"), "Ctrl+K");
        assert_eq!(pc("⏎ confirm  ⎋ cancel"), "Enter confirm  Esc cancel");
        assert_eq!(
            pc("(↑/↓ history, ⇥ complete)"),
            "(↑/↓ history, Tab complete)"
        );
        assert_eq!(
            pc("⌘O open · ⌘R refresh · ? help"),
            "Ctrl+O open · Ctrl+R refresh · ? help"
        );
        assert_eq!(pc("[ Open ⌘O ]"), "[ Open Ctrl+O ]");
        assert_eq!(pc("hold ⌘ to"), "hold Ctrl to");
        assert_eq!(pc("(⌘)"), "(Ctrl)");
    }

    #[test]
    fn text_without_glyphs_is_not_copied() {
        assert!(matches!(
            to_pc(Cow::Borrowed("plain ↑↓ ●")),
            Cow::Borrowed(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keeps_the_glyphs() {
        assert_eq!(local(Cow::Borrowed("⌘⇧K")), "⌘⇧K");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_spell_shortcuts_out() {
        assert_eq!(local(Cow::Borrowed("⌘⇧K")), "Ctrl+Shift+K");
    }
}
