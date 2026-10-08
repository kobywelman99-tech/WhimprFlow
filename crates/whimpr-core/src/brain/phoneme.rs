//! The 39 English phonemes (ARPAbet, as used by the CMU Pronouncing Dictionary)
//! and how acoustically close any two are.
//!
//! Closeness is the speech science the brain leans on:
//! - Vowels differ in their first two formants: F1 (how open the mouth is) and
//!   F2 (how far forward the tongue is). Values are rounded averages of adult male
//!   speakers from Peterson & Barney (1952); diphthongs use their starting target.
//! - Consonants differ in place of articulation (where airflow is shaped), manner
//!   (stop, fricative, nasal…) and voicing (whether the vocal folds vibrate).

/// A phoneme, as an index into [`PHONEMES`].
pub type Ph = u8;

pub const COUNT: usize = 39;

pub const PHONEMES: [&str; COUNT] = [
    "AA", "AE", "AH", "AO", "AW", "AY", "B", "CH", "D", "DH", "EH", "ER", "EY", "F", "G", "HH", "IH", "IY", "JH", "K",
    "L", "M", "N", "NG", "OW", "OY", "P", "R", "S", "SH", "T", "TH", "UH", "UW", "V", "W", "Y", "Z", "ZH",
];

/// Plain-English name with an example word, in the same order as [`PHONEMES`].
const LABELS: [&str; COUNT] = [
    "ah (father)", "a (cat)", "uh (but)", "aw (thought)", "ow (cow)", "eye (my)", "b (bat)", "ch (chin)", "d (dog)",
    "th (this)", "eh (bed)", "er (bird)", "ay (day)", "f (fan)", "g (go)", "h (hat)", "ih (sit)", "ee (see)",
    "j (jam)", "k (cat)", "l (let)", "m (man)", "n (net)", "ng (sing)", "oh (go)", "oy (boy)", "p (pat)", "r (red)",
    "s (sit)", "sh (ship)", "t (top)", "th (think)", "oo (book)", "oo (food)", "v (van)", "w (win)", "y (yes)",
    "z (zoo)", "zh (measure)",
];

/// Parse a CMUdict symbol such as `AH0` or `TH`; stress digits are dropped.
pub fn parse(sym: &str) -> Option<Ph> {
    let base = sym.trim_end_matches(|c: char| c.is_ascii_digit());
    PHONEMES.iter().position(|p| *p == base).map(|i| i as Ph)
}

pub fn name(p: Ph) -> &'static str {
    PHONEMES[p as usize]
}

pub fn label(p: Ph) -> &'static str {
    LABELS[p as usize]
}

/// (F1, F2) in Hz for vowels.
fn formants(sym: &str) -> Option<(f64, f64)> {
    Some(match sym {
        "AA" => (730.0, 1090.0),
        "AE" => (660.0, 1720.0),
        "AH" => (640.0, 1190.0),
        "AO" => (570.0, 840.0),
        "AW" => (700.0, 1200.0),
        "AY" => (700.0, 1400.0),
        "EH" => (530.0, 1840.0),
        "ER" => (490.0, 1350.0),
        "EY" => (450.0, 2000.0),
        "IH" => (390.0, 1990.0),
        "IY" => (270.0, 2290.0),
        "OW" => (500.0, 900.0),
        "OY" => (520.0, 1000.0),
        "UH" => (440.0, 1020.0),
        "UW" => (300.0, 870.0),
        _ => return None,
    })
}

/// (place 0–7 front to back, manner 0–3.5 by openness, voiced) for consonants.
fn consonant(sym: &str) -> Option<(f64, f64, bool)> {
    // place: bilabial 0, labiodental 1, dental 2, alveolar 3, postalveolar 4,
    // palatal 5, velar 6, glottal 7. manner: stop 0, affricate 0.5, fricative 1,
    // nasal 2, lateral 3, approximant 3.5.
    Some(match sym {
        "P" => (0.0, 0.0, false),
        "B" => (0.0, 0.0, true),
        "T" => (3.0, 0.0, false),
        "D" => (3.0, 0.0, true),
        "K" => (6.0, 0.0, false),
        "G" => (6.0, 0.0, true),
        "CH" => (4.0, 0.5, false),
        "JH" => (4.0, 0.5, true),
        "F" => (1.0, 1.0, false),
        "V" => (1.0, 1.0, true),
        "TH" => (2.0, 1.0, false),
        "DH" => (2.0, 1.0, true),
        "S" => (3.0, 1.0, false),
        "Z" => (3.0, 1.0, true),
        "SH" => (4.0, 1.0, false),
        "ZH" => (4.0, 1.0, true),
        "HH" => (7.0, 1.0, false),
        "M" => (0.0, 2.0, true),
        "N" => (3.0, 2.0, true),
        "NG" => (6.0, 2.0, true),
        "L" => (3.0, 3.0, true),
        "R" => (4.0, 3.5, true),
        "W" => (0.5, 3.5, true),
        "Y" => (5.0, 3.5, true),
        _ => return None,
    })
}

pub fn is_vowel(p: Ph) -> bool {
    formants(name(p)).is_some()
}

/// How far apart two phonemes sound: 0 for the same, about 0.3–1 for near
/// neighbours (th/f, s/z, ih/ee), and 3 across vowel/consonant.
pub fn distance(a: Ph, b: Ph) -> f64 {
    if a == b {
        return 0.0;
    }
    let (na, nb) = (name(a), name(b));
    match (formants(na), formants(nb)) {
        (Some((f1a, f2a)), Some((f1b, f2b))) => (((f1a - f1b) / 300.0).powi(2) + ((f2a - f2b) / 900.0).powi(2)).sqrt(),
        (None, None) => {
            let (pa, ma, va) = consonant(na).unwrap_or((0.0, 0.0, false));
            let (pb, mb, vb) = consonant(nb).unwrap_or((0.0, 0.0, false));
            (pa - pb).abs() * 0.25 + (ma - mb).abs() * 0.5 + if va != vb { 0.6 } else { 0.0 }
        }
        _ => 3.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ph(s: &str) -> Ph {
        parse(s).unwrap()
    }

    #[test]
    fn every_phoneme_has_features() {
        for p in PHONEMES {
            assert!(formants(p).is_some() || consonant(p).is_some(), "{p}");
        }
        assert_eq!(PHONEMES.len(), LABELS.len());
    }

    #[test]
    fn parse_drops_stress() {
        assert_eq!(parse("AH0"), parse("AH"));
        assert_eq!(parse("IY1"), Some(ph("IY")));
        assert_eq!(parse("QQ"), None);
    }

    #[test]
    fn near_sounds_are_closer_than_far_ones() {
        assert!(distance(ph("TH"), ph("F")) < distance(ph("TH"), ph("K")));
        assert!(distance(ph("S"), ph("Z")) < distance(ph("S"), ph("M")));
        assert!(distance(ph("IY"), ph("IH")) < distance(ph("IY"), ph("AA")));
        assert!(distance(ph("L"), ph("R")) < distance(ph("L"), ph("P")));
        assert_eq!(distance(ph("T"), ph("T")), 0.0);
        assert_eq!(distance(ph("T"), ph("IY")), 3.0);
    }

    #[test]
    fn distance_is_symmetric() {
        for a in 0..COUNT as Ph {
            for b in 0..COUNT as Ph {
                assert!((distance(a, b) - distance(b, a)).abs() < 1e-9);
            }
        }
    }
}
