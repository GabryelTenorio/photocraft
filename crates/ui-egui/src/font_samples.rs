//! Which script a font menu row samples, decided from the face's character map and the code
//! pages it declares.
//!
//! Font menus elsewhere show a face in the script it is for: Google Fonts specimens use a
//! family's `primary_script`, macOS Font Book samples the script of the user's language when the
//! font has it, and Photoshop previews non-Latin faces in their own script. Installed fonts carry
//! no primary script, so the face's coverage decides:
//!
//! - A script counts only when the face maps a few of its core letters and every character of
//!   its sample, so a face with a handful of stray glyphs is not mistaken for a script font.
//! - Chinese, Japanese and Korean fonts share the Han block and often map all of it, so their
//!   characters can't tell them apart. A face that declares CJK code pages (OS/2
//!   `ulCodePageRange`, which Windows uses to match fonts to languages) is sampled only in the
//!   CJK scripts it declares: Microsoft YaHei in Simplified Chinese, Malgun Gothic in Korean.
//! - Among the remaining scripts, the one with the most mapped characters in its Unicode blocks
//!   is the face's primary script; ties go to the earlier [`SCRIPTS`] entry. A pan-CJK face that
//!   declares every CJK code page, or none, is sampled in Japanese when it has half-width
//!   katakana (as Japanese fonts do), else in Simplified, then Traditional Chinese.
//! - Greek, Cyrillic, Armenian and Georgian are often added to Latin fonts (Cambria, Verdana),
//!   so they are shown only when they outweigh Latin; every other script is shown whenever it
//!   is covered.
//! - A face with Latin shows the Latin sample and the primary script's sample beside it. A face
//!   without Latin shows its primary script's sample alone.

/// A script the font menu can sample.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Script {
    #[cfg_attr(not(test), allow(dead_code, reason = "the tests name the scripts"))]
    name: &'static str,
    /// A short native sample, about as wide as four Latin letters at the menu's size.
    pub sample: &'static str,
    /// Letters (besides the sample's own) a face must map to cover the script.
    core: &'static [char],
    /// The script's Unicode blocks, which measure how much of the face it takes up.
    ranges: &'static [(char, char)],
    /// Scripts often added to Latin fonts: shown only when they outweigh Latin.
    companion: bool,
    /// The OS/2 code page bit of a CJK script.
    code_page: Option<u32>,
}

/// Han and kana, shared by the Japanese and Chinese entries.
const HAN_KANA: &[(char, char)] = &[
    ('\u{3040}', '\u{30FF}'),
    ('\u{31F0}', '\u{31FF}'),
    ('\u{3400}', '\u{4DBF}'),
    ('\u{4E00}', '\u{9FFF}'),
    ('\u{F900}', '\u{FAFF}'),
    ('\u{FF66}', '\u{FF9F}'),
];

/// OS/2 code page bits: 932 Japanese, 936 Simplified Chinese, 949 Korean, 950 Traditional Chinese.
const JAPANESE: u32 = 17;
const SIMPLIFIED_CHINESE: u32 = 18;
const KOREAN: u32 = 19;
const TRADITIONAL_CHINESE: u32 = 20;
const CJK_CODE_PAGES: u64 = 1 << JAPANESE | 1 << SIMPLIFIED_CHINESE | 1 << KOREAN | 1 << TRADITIONAL_CHINESE;

/// A script outside CJK that is shown whenever a face covers it.
const fn script(name: &'static str, sample: &'static str, core: &'static [char], ranges: &'static [(char, char)]) -> Script {
    Script { name, sample, core, ranges, companion: false, code_page: None }
}

/// The sampled scripts, in tie-break order.
pub(crate) const SCRIPTS: &[Script] = &[
    // Alef, lam, meem and ain; the sample is the start of the abjad order.
    script(
        "Arabic",
        "أبجد هوز",
        &['\u{627}', '\u{644}', '\u{645}', '\u{639}'],
        &[('\u{600}', '\u{6FF}'), ('\u{750}', '\u{77F}'), ('\u{870}', '\u{8FF}'), ('\u{FB50}', '\u{FDFF}'), ('\u{FE70}', '\u{FEFF}')],
    ),
    script("Hebrew", "אבגד", &['\u{5DC}', '\u{5E9}', '\u{5EA}'], &[('\u{590}', '\u{5FF}'), ('\u{FB1D}', '\u{FB4F}')]),
    // "Hindi": a conjunct and two vowel signs, so the sample shows the face's shaping.
    script("Devanagari", "हिन्दी", &['\u{915}', '\u{930}', '\u{93E}'], &[('\u{900}', '\u{97F}'), ('\u{A8E0}', '\u{A8FF}')]),
    script("Bengali", "বাংলা", &['\u{995}', '\u{9B0}'], &[('\u{980}', '\u{9FF}')]),
    script("Tamil", "தமிழ்", &['\u{B95}', '\u{BA9}'], &[('\u{B80}', '\u{BFF}')]),
    script("Thai", "สวัสดี", &['\u{E01}', '\u{E32}'], &[('\u{E00}', '\u{E7F}')]),
    // Hiragana, katakana and kanji; half-width katakana (JIS X 0201) marks a Japanese face.
    Script { code_page: Some(JAPANESE), ..script("Japanese", "あア亜", &['\u{306E}', '\u{30F3}', '\u{65E5}', '\u{FF71}'], HAN_KANA) },
    // 这 and 语 are simplified forms that Traditional Chinese fonts lack.
    Script { code_page: Some(SIMPLIFIED_CHINESE), ..script("Simplified Chinese", "汉字", &['\u{4E2D}', '\u{8FD9}', '\u{8BED}'], HAN_KANA) },
    Script { code_page: Some(TRADITIONAL_CHINESE), ..script("Traditional Chinese", "漢字", &['\u{4E2D}', '\u{9019}', '\u{8A9E}'], HAN_KANA) },
    Script {
        code_page: Some(KOREAN),
        ..script("Korean", "가나다", &['\u{D55C}', '\u{AE00}'], &[('\u{1100}', '\u{11FF}'), ('\u{3130}', '\u{318F}'), ('\u{AC00}', '\u{D7AF}')])
    },
    script("Ethiopic", "ሀለሐ", &['\u{1218}', '\u{12A0}'], &[('\u{1200}', '\u{139F}'), ('\u{2D80}', '\u{2DDF}'), ('\u{AB00}', '\u{AB2F}')]),
    Script { companion: true, ..script("Armenian", "ԱաԲբ", &['\u{576}', '\u{561}'], &[('\u{530}', '\u{58F}'), ('\u{FB13}', '\u{FB17}')]) },
    Script {
        companion: true,
        ..script("Georgian", "აბგდ", &['\u{10D4}', '\u{10D0}'], &[('\u{10A0}', '\u{10FF}'), ('\u{1C90}', '\u{1CBF}'), ('\u{2D00}', '\u{2D2F}')])
    },
    // The basic block only: polytonic Greek would let a Latin font's extras outweigh its Latin.
    Script { companion: true, ..script("Greek", "ΑαΒβ", &['\u{3A9}', '\u{3C9}', '\u{3BB}'], &[('\u{370}', '\u{3FF}')]) },
    Script {
        companion: true,
        ..script(
            "Cyrillic",
            "АаБб",
            &['\u{42F}', '\u{44F}', '\u{436}'],
            &[('\u{400}', '\u{52F}'), ('\u{1C80}', '\u{1C8F}'), ('\u{2DE0}', '\u{2DFF}'), ('\u{A640}', '\u{A69F}')],
        )
    },
];

/// Latin letters (the weight companion scripts must exceed).
const LATIN: &[(char, char)] = &[('A', 'Z'), ('a', 'z'), ('\u{C0}', '\u{24F}'), ('\u{1E00}', '\u{1EFF}')];

/// How many characters of the ascending `chars` fall in `ranges`.
fn count(chars: &[char], ranges: &[(char, char)]) -> usize {
    ranges.iter().map(|&(lo, hi)| chars.partition_point(|&c| c <= hi).saturating_sub(chars.partition_point(|&c| c < lo))).sum()
}

fn covers(chars: &[char], text: impl IntoIterator<Item = char>) -> bool {
    text.into_iter().filter(|c| !c.is_whitespace()).all(|c| chars.binary_search(&c).is_ok())
}

/// The scripts a face covers. When it declares CJK code pages, CJK scripts it doesn't declare
/// are left out, unless that would leave out every CJK script it covers.
fn covered(chars: &[char], code_pages: u64) -> Vec<&'static Script> {
    let all: Vec<&'static Script> = SCRIPTS.iter().filter(|s| covers(chars, s.core.iter().copied().chain(s.sample.chars()))).collect();
    let declared = code_pages & CJK_CODE_PAGES;
    let undeclared = |s: &Script| s.code_page.is_some_and(|bit| declared & (1 << bit) == 0);
    let narrowed: Vec<&'static Script> = all.iter().copied().filter(|s| declared == 0 || !undeclared(s)).collect();
    let cjk = |list: &[&Script]| list.iter().any(|s| s.code_page.is_some());
    if cjk(&all) && !cjk(&narrowed) { all } else { narrowed }
}

/// The script a face is sampled in besides Latin (`latin`: the face has the Latin sample), or
/// alone (`!latin`). `chars` is the face's ascending charmap and `code_pages` its OS/2 code page
/// bits ([`photocraft_text::FontDb::code_pages`]). See the module docs for the rule.
pub(crate) fn primary(chars: &[char], latin: bool, code_pages: u64) -> Option<&'static Script> {
    let weighed: Vec<(&'static Script, usize)> = covered(chars, code_pages).into_iter().map(|s| (s, count(chars, s.ranges))).collect();
    // `max_by_key` keeps the last maximum: searching the table backwards gives ties to the earlier entry.
    let heaviest = |companion: Option<bool>| weighed.iter().rev().filter(|(s, _)| companion.is_none_or(|c| s.companion == c)).max_by_key(|&&(_, n)| n).copied();
    let best = if latin { heaviest(Some(false)).or_else(|| heaviest(Some(true)).filter(|&(_, n)| n > count(chars, LATIN))) } else { heaviest(None) };
    best.map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ascending charmap of `text` plus every character of the given blocks.
    fn charmap(text: &str, blocks: &[(char, char)]) -> Vec<char> {
        let mut chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        for &(lo, hi) in blocks {
            chars.extend((lo..=hi).filter(|c| !c.is_whitespace()));
        }
        chars.sort_unstable();
        chars.dedup();
        chars
    }

    fn script(name: &str) -> &'static Script {
        SCRIPTS.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{name} in the table"))
    }

    /// Every character a face needs to cover `name`.
    fn letters(name: &str) -> String {
        let s = script(name);
        s.core.iter().copied().chain(s.sample.chars()).collect()
    }

    fn primary_name(chars: &[char], latin: bool) -> Option<&'static str> {
        primary(chars, latin, 0).map(|s| s.name)
    }

    /// The second script of a Latin face declaring Latin 1 and the code pages `bits`.
    fn declared(chars: &[char], bits: &[u32]) -> Option<&'static str> {
        primary(chars, true, bits.iter().fold(1, |acc, bit| acc | 1 << bit)).map(|s| s.name)
    }

    const ASCII: (char, char) = ('!', '~');

    #[test]
    fn empty_and_latin_only_faces_have_no_second_script() {
        assert_eq!(primary_name(&[], true), None);
        assert_eq!(primary_name(&[], false), None);
        let latin = charmap("", &[ASCII, ('\u{A0}', '\u{17F}')]);
        assert_eq!(primary_name(&latin, true), None);
    }

    #[test]
    fn every_script_is_chosen_when_it_is_the_only_one() {
        for s in SCRIPTS {
            let chars = charmap(&letters(s.name), &[]);
            assert_eq!(primary_name(&chars, false), Some(s.name), "{} alone", s.name);
            if !s.companion {
                let with_latin = charmap(&letters(s.name), &[ASCII]);
                assert_eq!(primary_name(&with_latin, true), Some(s.name), "{} with Latin", s.name);
            }
        }
    }

    #[test]
    fn a_missing_core_or_sample_letter_drops_the_script() {
        for s in SCRIPTS {
            for missing in s.core.iter().copied().chain(s.sample.chars().filter(|c| !c.is_whitespace())) {
                let chars: Vec<char> = charmap(&letters(s.name), &[ASCII]).into_iter().filter(|&c| c != missing).collect();
                assert_ne!(primary_name(&chars, true), Some(s.name), "{} without U+{:04X}", s.name, missing as u32);
            }
        }
    }

    /// Arial, Segoe UI and Tahoma cover Arabic, Hebrew, Greek and Cyrillic (and Thai): the
    /// Arabic sample they showed before stays.
    #[test]
    fn arabic_and_latin_faces_keep_the_arabic_sample() {
        let blocks = [
            ASCII,
            ('\u{C0}', '\u{17F}'),
            ('\u{370}', '\u{3FF}'),
            ('\u{400}', '\u{4FF}'),
            ('\u{5D0}', '\u{5EA}'),
            ('\u{600}', '\u{6FF}'),
            ('\u{FE70}', '\u{FEFC}'),
        ];
        let text = format!("{}{}", letters("Hebrew"), letters("Thai"));
        assert_eq!(primary_name(&charmap(&text, &blocks), true), Some("Arabic"));
    }

    #[test]
    fn cjk_faces_are_sampled_in_their_own_script() {
        let han = ('\u{4E00}', '\u{9FFF}');
        let kana = ('\u{3041}', '\u{30FF}');
        let half_width = ('\u{FF66}', '\u{FF9F}');
        let hangul = ('\u{AC00}', '\u{D7A3}');
        // A Japanese face: JIS kanji, kana and half-width katakana.
        assert_eq!(primary_name(&charmap("", &[ASCII, ('\u{4E00}', '\u{7FFF}'), kana, half_width]), true), Some("Japanese"));
        // A Simplified Chinese face (GBK): all URO Han and kana, no half-width katakana.
        assert_eq!(primary_name(&charmap("", &[ASCII, han, kana]), true), Some("Simplified Chinese"));
        // A Traditional Chinese face (Big5): no simplified-only forms.
        let traditional: Vec<char> = charmap("", &[ASCII, han]).into_iter().filter(|c| !['\u{8FD9}', '\u{8BED}', '\u{6C49}'].contains(c)).collect();
        assert_eq!(primary_name(&traditional, true), Some("Traditional Chinese"));
        // A Korean face: every Hangul syllable outweighs its few thousand hanja.
        assert_eq!(primary_name(&charmap(&letters("Japanese"), &[ASCII, ('\u{4E00}', '\u{5FFF}'), hangul]), true), Some("Korean"));
        // CJK without Latin: the CJK sample alone.
        assert_eq!(primary_name(&charmap("", &[han, kana, half_width]), false), Some("Japanese"));
    }

    /// Microsoft YaHei, JhengHei and Malgun Gothic map all of the Han block (and YaHei half-width
    /// katakana too): their declared code pages decide.
    #[test]
    fn declared_cjk_code_pages_pick_among_cjk_scripts() {
        let han = ('\u{4E00}', '\u{9FFF}');
        let everything = charmap("", &[ASCII, han, ('\u{3041}', '\u{30FF}'), ('\u{FF66}', '\u{FF9F}'), ('\u{AC00}', '\u{D7A3}')]);
        assert_eq!(declared(&everything, &[SIMPLIFIED_CHINESE]), Some("Simplified Chinese"), "YaHei");
        assert_eq!(declared(&everything, &[TRADITIONAL_CHINESE]), Some("Traditional Chinese"), "JhengHei");
        assert_eq!(declared(&everything, &[KOREAN]), Some("Korean"), "Malgun Gothic, though Han outweighs Hangul");
        assert_eq!(declared(&everything, &[JAPANESE]), Some("Japanese"), "Yu Gothic");
        assert_eq!(declared(&everything, &[JAPANESE, SIMPLIFIED_CHINESE, KOREAN, TRADITIONAL_CHINESE]), Some("Japanese"), "pan-CJK");
        assert_eq!(declared(&everything, &[]), Some("Japanese"), "Latin 1 only: the characters decide");
        // A declared script the face doesn't cover leaves the covered ones in play.
        let no_half_width: Vec<char> = everything.iter().copied().filter(|c| !('\u{FF66}'..='\u{FF9F}').contains(c)).collect();
        assert_eq!(declared(&no_half_width, &[JAPANESE]), Some("Simplified Chinese"));
        // Code pages don't touch other scripts.
        assert_eq!(declared(&charmap(&letters("Hebrew"), &[ASCII]), &[SIMPLIFIED_CHINESE]), Some("Hebrew"));
    }

    /// Noto Sans CJK and Arial Unicode cover nearly everything: Han outweighs the rest, and the
    /// Japanese entry is first of the Han scripts.
    #[test]
    fn a_face_covering_many_scripts_shows_the_one_it_covers_most() {
        let all = [
            ASCII,
            ('\u{370}', '\u{52F}'),
            ('\u{590}', '\u{6FF}'),
            ('\u{900}', '\u{E7F}'),
            ('\u{3041}', '\u{30FF}'),
            ('\u{4E00}', '\u{9FFF}'),
            ('\u{AC00}', '\u{D7A3}'),
            ('\u{FF66}', '\u{FF9F}'),
        ];
        let text: String = SCRIPTS.iter().map(|s| letters(s.name)).collect();
        assert_eq!(primary_name(&charmap(&text, &all), true), Some("Japanese"));
        // Nirmala UI: the Indic blocks weigh about the same, and Devanagari is listed first.
        let indic = charmap(&format!("{}{}", letters("Bengali"), letters("Tamil")), &[ASCII, ('\u{900}', '\u{BFF}')]);
        assert_eq!(primary_name(&indic, true), Some("Devanagari"));
    }

    #[test]
    fn companion_scripts_show_only_when_they_outweigh_latin() {
        let lgc = charmap(&letters("Cyrillic"), &[ASCII, ('\u{C0}', '\u{24F}'), ('\u{370}', '\u{3FF}'), ('\u{400}', '\u{45F}')]);
        assert_eq!(primary_name(&lgc, true), None, "a Latin font with Greek and Cyrillic stays Latin");
        let cambria = charmap(&format!("{}{}", letters("Armenian"), letters("Georgian")), &[ASCII, ('\u{C0}', '\u{24F}'), ('\u{530}', '\u{58F}')]);
        assert_eq!(primary_name(&cambria, true), None, "nor with Armenian and Georgian");
        let gabriola = charmap("", &[ASCII, ('\u{C0}', '\u{17F}'), ('\u{370}', '\u{3FF}'), ('\u{1F00}', '\u{1FFF}')]);
        assert_eq!(primary_name(&gabriola, true), None, "polytonic Greek doesn't count");
        let cyrillic_font = charmap("", &[ASCII, ('\u{400}', '\u{52F}')]);
        assert_eq!(primary_name(&cyrillic_font, true), Some("Cyrillic"), "Cyrillic outweighs basic Latin");
        let greek_only = charmap("", &[('\u{370}', '\u{3FF}')]);
        assert_eq!(primary_name(&greek_only, false), Some("Greek"));
        // Any other script wins over a heavier companion.
        let mixed = charmap(&letters("Hebrew"), &[ASCII, ('\u{400}', '\u{52F}')]);
        assert_eq!(primary_name(&mixed, true), Some("Hebrew"));
    }

    #[test]
    fn counting_is_by_block() {
        let chars = charmap("abc", &[('\u{600}', '\u{60F}')]);
        assert_eq!(count(&chars, LATIN), 3);
        assert_eq!(count(&chars, script("Arabic").ranges), 16);
        assert_eq!(count(&[], LATIN), 0);
    }
}
