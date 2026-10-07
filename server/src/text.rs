//! Lightweight, dependency-free text post-processing for transcripts:
//! spoken-number normalization, punctuation stripping and profanity masking.
//! Number parsing is best-effort and covers en/fr/es/de/it; other languages
//! pass through unchanged.

use std::collections::HashMap;

// ── Spoken numbers → digits ──────────────────────────────────────────────────
// Cardinal parsing: "one hundred twenty three" → "123", "vingt-trois" → "23".
// Digit-dictation ("one two three") is parsed as a single cardinal, a known
// trade-off of cardinal normalization.

fn flush_num(out: &mut Vec<String>, result: &mut u64, current: &mut u64, in_num: &mut bool) {
    if *in_num {
        out.push((*result + *current).to_string());
        *result = 0;
        *current = 0;
        *in_num = false;
    }
}

/// (small words < 100, scale words: 100/1000/…). Returns None for unsupported langs.
fn num_map(lang: &str) -> Option<(HashMap<&'static str, u64>, HashMap<&'static str, u64>)> {
    let (smalls, scales): (&[(&str, u64)], &[(&str, u64)]) = match lang {
        "en" => (
            &[("zero",0),("one",1),("two",2),("three",3),("four",4),("five",5),("six",6),("seven",7),
              ("eight",8),("nine",9),("ten",10),("eleven",11),("twelve",12),("thirteen",13),("fourteen",14),
              ("fifteen",15),("sixteen",16),("seventeen",17),("eighteen",18),("nineteen",19),("twenty",20),
              ("thirty",30),("forty",40),("fifty",50),("sixty",60),("seventy",70),("eighty",80),("ninety",90)],
            &[("hundred",100),("thousand",1000),("million",1_000_000),("billion",1_000_000_000)],
        ),
        "fr" => (
            &[("zéro",0),("zero",0),("un",1),("une",1),("deux",2),("trois",3),("quatre",4),("cinq",5),
              ("six",6),("sept",7),("huit",8),("neuf",9),("dix",10),("onze",11),("douze",12),("treize",13),
              ("quatorze",14),("quinze",15),("seize",16),("dix-sept",17),("dix-huit",18),("dix-neuf",19),
              ("vingt",20),("vingts",20),("trente",30),("quarante",40),("cinquante",50),("soixante",60),
              ("soixante-dix",70),("quatre-vingt",80),("quatre-vingts",80),("quatre-vingt-dix",90)],
            &[("cent",100),("cents",100),("mille",1000),("million",1_000_000),("milliard",1_000_000_000)],
        ),
        "es" => (
            &[("cero",0),("uno",1),("una",1),("un",1),("dos",2),("tres",3),("cuatro",4),("cinco",5),
              ("seis",6),("siete",7),("ocho",8),("nueve",9),("diez",10),("once",11),("doce",12),("trece",13),
              ("catorce",14),("quince",15),("dieciséis",16),("diecisiete",17),("dieciocho",18),("diecinueve",19),
              ("veinte",20),("treinta",30),("cuarenta",40),("cincuenta",50),("sesenta",60),("setenta",70),
              ("ochenta",80),("noventa",90)],
            &[("cien",100),("ciento",100),("mil",1000),("millón",1_000_000)],
        ),
        "de" => (
            &[("null",0),("eins",1),("ein",1),("eine",1),("zwei",2),("drei",3),("vier",4),("fünf",5),
              ("sechs",6),("sieben",7),("acht",8),("neun",9),("zehn",10),("elf",11),("zwölf",12),("dreizehn",13),
              ("vierzehn",14),("fünfzehn",15),("sechzehn",16),("siebzehn",17),("achtzehn",18),("neunzehn",19),
              ("zwanzig",20),("dreißig",30),("vierzig",40),("fünfzig",50),("sechzig",60),("siebzig",70),
              ("achtzig",80),("neunzig",90)],
            &[("hundert",100),("tausend",1000),("million",1_000_000)],
        ),
        "it" => (
            &[("zero",0),("uno",1),("una",1),("due",2),("tre",3),("quattro",4),("cinque",5),("sei",6),
              ("sette",7),("otto",8),("nove",9),("dieci",10),("undici",11),("dodici",12),("tredici",13),
              ("quattordici",14),("quindici",15),("sedici",16),("diciassette",17),("diciotto",18),("diciannove",19),
              ("venti",20),("trenta",30),("quaranta",40),("cinquanta",50),("sessanta",60),("settanta",70),
              ("ottanta",80),("novanta",90)],
            &[("cento",100),("mille",1000),("mila",1000),("milione",1_000_000)],
        ),
        _ => return None,
    };
    Some((smalls.iter().copied().collect(), scales.iter().copied().collect()))
}

/// Apply one number word to the accumulator. Returns false if it isn't a number word.
fn apply_word(
    key: &str,
    smalls: &HashMap<&str, u64>, scales: &HashMap<&str, u64>,
    current: &mut u64, result: &mut u64, in_num: &mut bool,
) -> bool {
    if let Some(&v) = smalls.get(key) {
        *current += v;
        *in_num = true;
        true
    } else if let Some(&s) = scales.get(key) {
        if s == 100 {
            *current = if *current == 0 { 1 } else { *current } * 100;
        } else {
            *result += if *current == 0 { 1 } else { *current } * s;
            *current = 0;
        }
        *in_num = true;
        true
    } else {
        false
    }
}

/// Convert spoken number words to digits where possible. Non-number words pass through.
pub fn normalize_numbers(text: &str, lang: &str) -> String {
    let Some((smalls, scales)) = num_map(lang) else { return text.to_string() };
    let mut out: Vec<String> = Vec::new();
    let (mut current, mut result, mut in_num) = (0u64, 0u64, false);
    for raw in text.split_whitespace() {
        // Keep letters + hyphens (for "quatre-vingt"); lowercase for lookup.
        let key: String = raw.chars().filter(|c| c.is_alphabetic() || *c == '-').collect::<String>().to_lowercase();
        // Filler inside numbers ("one hundred and two", "cent et un").
        if (key == "and" || key == "et") && in_num { continue; }

        // Try the whole token first (so "quatre-vingt"=80 wins over 4+20), then
        // fall back to decomposing a hyphenated compound ("vingt-trois" → 20+3).
        if apply_word(&key, &smalls, &scales, &mut current, &mut result, &mut in_num) {
            continue;
        }
        if key.contains('-') {
            let parts: Vec<&str> = key.split('-').filter(|p| !p.is_empty()).collect();
            let all_numbers = !parts.is_empty()
                && parts.iter().all(|p| smalls.contains_key(p) || scales.contains_key(p) || *p == "et");
            if all_numbers {
                for p in parts {
                    if p == "et" { continue; }
                    apply_word(p, &smalls, &scales, &mut current, &mut result, &mut in_num);
                }
                continue;
            }
        }
        flush_num(&mut out, &mut result, &mut current, &mut in_num);
        out.push(raw.to_string());
    }
    flush_num(&mut out, &mut result, &mut current, &mut in_num);
    out.join(" ")
}

// ── Punctuation ──────────────────────────────────────────────────────────────
pub fn strip_punctuation(text: &str) -> String {
    let t: String = text
        .chars()
        .filter(|c| !matches!(c,
            '.' | ',' | ';' | ':' | '!' | '?' | '…'
            | '。' | '，' | '、' | '？' | '！' | '；' | '：' | '؟' | '،'))
        .collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Profanity masking ────────────────────────────────────────────────────────
fn profanity(lang: &str) -> &'static [&'static str] {
    match lang {
        "fr" => &["merde", "putain", "connard", "connasse", "salope", "enculé", "enculer", "bite", "couilles", "pute"],
        _ => &["fuck", "fucking", "shit", "bitch", "asshole", "cunt", "dick", "bastard", "motherfucker"],
    }
}

fn mask(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => {
            let rest = word.chars().count().saturating_sub(1);
            format!("{first}{}", "*".repeat(rest))
        }
        None => String::new(),
    }
}

pub fn filter_profanity(text: &str, lang: &str) -> String {
    let list = profanity(lang);
    text.split_whitespace()
        .map(|w| {
            let core: String = w.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
            if list.contains(&core.as_str()) { mask(w) } else { w.to_string() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn en_numbers() {
        assert_eq!(normalize_numbers("one hundred twenty three apples", "en"), "123 apples");
        assert_eq!(normalize_numbers("two thousand", "en"), "2000");
        assert_eq!(normalize_numbers("twenty", "en"), "20");
    }
    #[test]
    fn fr_numbers() {
        assert_eq!(normalize_numbers("vingt-trois", "fr"), "23");
        assert_eq!(normalize_numbers("cent deux", "fr"), "102");
    }
    #[test]
    fn punctuation() {
        assert_eq!(strip_punctuation("Hello, world!"), "Hello world");
    }
    #[test]
    fn profanity_mask() {
        assert_eq!(filter_profanity("you shit head", "en"), "you s*** head");
    }
}
