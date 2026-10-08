//! Word-level alignment of expected vs heard text.

/// A span the recognizer got wrong: it heard `heard` where the user meant `expected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mishear {
    pub heard: String,
    pub expected: String,
}

/// Longest span (in words, per side) we treat as one mishear; longer runs are
/// usually a skipped or garbled phrase rather than a pronunciation pattern.
const MAX_SPAN: usize = 3;

pub(crate) fn tokens(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}')
                .replace('\u{2019}', "'")
        })
        .filter(|t| !t.is_empty())
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Match,
    Sub,
    Del, // expected word with no heard counterpart
    Ins, // heard word with no expected counterpart
}

/// One stretch of an alignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A word both sides agree on (ignoring case/punctuation), as spelled in `expected`.
    Same(String),
    /// A run where they disagree. `one_to_one` means every word was a plain
    /// substitution (no insertions or deletions), so the words pair up by position.
    Differ { expected: Vec<String>, heard: Vec<String>, one_to_one: bool },
}

/// Align `heard` against `expected` word by word.
pub fn segments(expected: &str, heard: &str) -> Vec<Segment> {
    let e = tokens(expected);
    let h = tokens(heard);
    let eq = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();

    let (n, m) = (e.len(), h.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[i - 1][j - 1] + usize::from(!eq(&e[i - 1], &h[j - 1]));
            d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }

    // Backtrace into ops (reversed), pairing each with its (expected, heard) word.
    let (mut i, mut j) = (n, m);
    let mut ops: Vec<(Op, Option<&str>, Option<&str>)> = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let same = eq(&e[i - 1], &h[j - 1]);
            if d[i][j] == d[i - 1][j - 1] + usize::from(!same) {
                ops.push((if same { Op::Match } else { Op::Sub }, Some(&e[i - 1]), Some(&h[j - 1])));
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && d[i][j] == d[i - 1][j] + 1 {
            ops.push((Op::Del, Some(&e[i - 1]), None));
            i -= 1;
        } else {
            ops.push((Op::Ins, None, Some(&h[j - 1])));
            j -= 1;
        }
    }
    ops.reverse();

    let mut out = Vec::new();
    let mut k = 0;
    while k < ops.len() {
        if ops[k].0 == Op::Match {
            out.push(Segment::Same(ops[k].1.unwrap_or_default().to_string()));
            k += 1;
            continue;
        }
        let mut exp: Vec<String> = Vec::new();
        let mut hrd: Vec<String> = Vec::new();
        let mut one_to_one = true;
        while k < ops.len() && ops[k].0 != Op::Match {
            one_to_one &= ops[k].0 == Op::Sub;
            exp.extend(ops[k].1.map(String::from));
            hrd.extend(ops[k].2.map(String::from));
            k += 1;
        }
        out.push(Segment::Differ { expected: exp, heard: hrd, one_to_one });
    }
    out
}

/// Diff `heard` against `expected`, returning substitution spans where both sides
/// are non-empty and at most [`MAX_SPAN`] words. Case and punctuation are ignored
/// when deciding equality; `expected` keeps its original casing.
pub fn diff(expected: &str, heard: &str) -> Vec<Mishear> {
    let mut out = Vec::new();
    for seg in segments(expected, heard) {
        let Segment::Differ { expected: exp, heard: hrd, one_to_one } = seg else {
            continue;
        };
        // Back-to-back one-for-one substitutions are separate word-level mishears.
        if one_to_one && (2..=MAX_SPAN).contains(&exp.len()) {
            for (h, e) in hrd.iter().zip(&exp) {
                out.push(Mishear { heard: h.to_lowercase(), expected: e.clone() });
            }
        } else if (1..=MAX_SPAN).contains(&exp.len()) && (1..=MAX_SPAN).contains(&hrd.len()) {
            out.push(Mishear { heard: hrd.join(" ").to_lowercase(), expected: exp.join(" ") });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(h: &str, e: &str) -> Mishear {
        Mishear { heard: h.into(), expected: e.into() }
    }

    #[test]
    fn identical_ignoring_case_and_punctuation() {
        assert!(diff("Hello, world.", "hello world").is_empty());
    }

    #[test]
    fn single_substitution() {
        assert_eq!(diff("the water is cold", "the wooder is cold"), vec![pair("wooder", "water")]);
    }

    #[test]
    fn split_word() {
        assert_eq!(diff("renew ChargeBee today", "renew charge bee today"), vec![pair("charge bee", "ChargeBee")]);
    }

    #[test]
    fn merged_words() {
        assert_eq!(diff("a lot of people", "alot of people"), vec![pair("alot", "a lot")]);
    }

    #[test]
    fn pure_insertion_or_deletion_is_ignored() {
        assert!(diff("one two three", "one three").is_empty());
        assert!(diff("one three", "one two three").is_empty());
    }

    #[test]
    fn long_garble_is_ignored() {
        assert!(diff("alpha beta gamma delta epsilon end", "xx yy zz ww vv end").is_empty());
    }

    #[test]
    fn multiple_spans() {
        let d = diff("I think this is rather lovely", "I fink dis is rather lovely");
        assert_eq!(d, vec![pair("fink", "think"), pair("dis", "this")]);
    }
}
