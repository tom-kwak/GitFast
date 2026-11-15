//! Recovers commands typed while the Korean (2-set / 두벌식) input method was
//! active: `ㅣㅐㅎ` → `log`, `햣` → `git`. Quoted text is left alone so
//! Korean commit messages survive.

const INITIALS: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ',
    'ㅌ', 'ㅍ', 'ㅎ',
];
const MEDIALS: [char; 21] = [
    'ㅏ', 'ㅐ', 'ㅑ', 'ㅒ', 'ㅓ', 'ㅔ', 'ㅕ', 'ㅖ', 'ㅗ', 'ㅘ', 'ㅙ', 'ㅚ', 'ㅛ', 'ㅜ', 'ㅝ', 'ㅞ',
    'ㅟ', 'ㅠ', 'ㅡ', 'ㅢ', 'ㅣ',
];
/// Index 0 = no final consonant.
const FINALS: [Option<char>; 28] = [
    None,
    Some('ㄱ'),
    Some('ㄲ'),
    Some('ㄳ'),
    Some('ㄴ'),
    Some('ㄵ'),
    Some('ㄶ'),
    Some('ㄷ'),
    Some('ㄹ'),
    Some('ㄺ'),
    Some('ㄻ'),
    Some('ㄼ'),
    Some('ㄽ'),
    Some('ㄾ'),
    Some('ㄿ'),
    Some('ㅀ'),
    Some('ㅁ'),
    Some('ㅂ'),
    Some('ㅄ'),
    Some('ㅅ'),
    Some('ㅆ'),
    Some('ㅇ'),
    Some('ㅈ'),
    Some('ㅊ'),
    Some('ㅋ'),
    Some('ㅌ'),
    Some('ㅍ'),
    Some('ㅎ'),
];

/// Keys that produce a compatibility jamo on the 2-set layout.
fn keys(jamo: char) -> Option<&'static str> {
    Some(match jamo {
        'ㅂ' => "q",
        'ㅈ' => "w",
        'ㄷ' => "e",
        'ㄱ' => "r",
        'ㅅ' => "t",
        'ㅛ' => "y",
        'ㅕ' => "u",
        'ㅑ' => "i",
        'ㅐ' => "o",
        'ㅔ' => "p",
        'ㅁ' => "a",
        'ㄴ' => "s",
        'ㅇ' => "d",
        'ㄹ' => "f",
        'ㅎ' => "g",
        'ㅗ' => "h",
        'ㅓ' => "j",
        'ㅏ' => "k",
        'ㅣ' => "l",
        'ㅋ' => "z",
        'ㅌ' => "x",
        'ㅊ' => "c",
        'ㅍ' => "v",
        'ㅠ' => "b",
        'ㅜ' => "n",
        'ㅡ' => "m",
        'ㅃ' => "Q",
        'ㅉ' => "W",
        'ㄸ' => "E",
        'ㄲ' => "R",
        'ㅆ' => "T",
        'ㅒ' => "O",
        'ㅖ' => "P",
        'ㅘ' => "hk",
        'ㅙ' => "ho",
        'ㅚ' => "hl",
        'ㅝ' => "nj",
        'ㅞ' => "np",
        'ㅟ' => "nl",
        'ㅢ' => "ml",
        'ㄳ' => "rt",
        'ㄵ' => "sw",
        'ㄶ' => "sg",
        'ㄺ' => "fr",
        'ㄻ' => "fa",
        'ㄼ' => "fq",
        'ㄽ' => "ft",
        'ㄾ' => "fx",
        'ㄿ' => "fv",
        'ㅀ' => "fg",
        'ㅄ' => "qt",
        _ => return None,
    })
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{3131}'..='\u{3163}').contains(&c)
}

/// Converts one character typed on the Korean layout back to its keys.
fn char_to_keys(c: char, out: &mut String) -> bool {
    if ('\u{AC00}'..='\u{D7A3}').contains(&c) {
        let code = u32::from(c) - 0xAC00;
        let (i, m, f) = (
            (code / 588) as usize,
            ((code % 588) / 28) as usize,
            (code % 28) as usize,
        );
        for jamo in [Some(INITIALS[i]), Some(MEDIALS[m]), FINALS[f]]
            .into_iter()
            .flatten()
        {
            match keys(jamo) {
                Some(k) => out.push_str(k),
                None => return false,
            }
        }
        true
    } else if let Some(k) = keys(c) {
        out.push_str(k);
        true
    } else {
        false
    }
}

/// Converts Hangul outside quotes to the keys that produced it. `None`
/// when the line contains no unquoted Hangul.
pub fn to_qwerty(line: &str) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    let mut converted = false;
    for c in line.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                out.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                out.push(c);
            }
            (None, c) if is_hangul(c) => {
                if !char_to_keys(c, &mut out) {
                    out.push(c);
                } else {
                    converted = true;
                }
            }
            _ => out.push(c),
        }
    }
    converted.then_some(out)
}

/// If the first word was typed in Korean mode and converts to something gf
/// knows (`known` decides), returns the converted line.
pub fn recover(line: &str, known: impl Fn(&str) -> bool) -> Option<String> {
    let first = line.split_whitespace().next()?;
    if !first.chars().any(is_hangul) {
        return None;
    }
    let converted = to_qwerty(line)?;
    let first = converted.split_whitespace().next()?;
    known(first).then_some(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_jamo_and_syllables() {
        assert_eq!(to_qwerty("ㅣㅐㅎ").as_deref(), Some("log"));
        assert_eq!(to_qwerty("햣").as_deref(), Some("git"));
        assert_eq!(
            to_qwerty("ㅣㅐㅎ --ㅐㅜ디ㅑㅜㄷ -3").as_deref(),
            Some("log --oneline -3")
        );
        assert_eq!(to_qwerty("ㄴㅅㅁ셔ㄴ").as_deref(), Some("status"));
        // Compound vowel and final consonant.
        assert_eq!(to_qwerty("뭡").as_deref(), Some("anjq"));
        assert_eq!(to_qwerty("status"), None);
    }

    #[test]
    fn keeps_quoted_korean() {
        assert_eq!(
            to_qwerty("채ㅡㅡㅑㅅ -m \"한글 메시지\"").as_deref(),
            Some("commit -m \"한글 메시지\"")
        );
    }

    #[test]
    fn recovers_only_known_commands() {
        let known = |w: &str| ["log", "status", "git"].contains(&w);
        assert_eq!(recover("ㅣㅐㅎ -3", known).as_deref(), Some("log -3"));
        assert_eq!(
            recover("안녕", known),
            None,
            "converts to dkssud, not a command"
        );
        assert_eq!(recover("log -3", known), None);
    }
}
