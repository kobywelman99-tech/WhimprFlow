//! A model of how this user's phonemes come out of the recognizer.
//!
//! Counts of what was meant vs what was heard are stored in [`Stats`]. A [`Model`]
//! turns them into probabilities, blending in a prior from acoustic closeness
//! (`phoneme::distance`) so sounds the user has never confused are still treated
//! as more or less plausible mix-ups. The prior shrinks as evidence arrives.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::phoneme::{self, Ph, COUNT};

/// Pseudo-observations behind the prior rates: more means slower to trust a few samples.
const PRIOR_WEIGHT: f64 = 20.0;
/// How often any one phoneme is misheard, before we know anything about the user.
const BASE_CONFUSION: f64 = 0.03;
const BASE_DELETION: f64 = 0.01;
const BASE_INSERTION: f64 = 0.004;
/// Pseudo-observations behind the "which sound instead" split.
const SHARE_WEIGHT: f64 = 3.0;
/// Cost of an insertion or deletion when aligning two pronunciations for learning.
const INDEL_ALIGN_COST: f64 = 1.5;

/// Raw counts, saved to disk. Keys are phoneme names; substitutions are "TH>F".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Times each phoneme was expected.
    pub opportunities: BTreeMap<String, u32>,
    pub substitutions: BTreeMap<String, u32>,
    pub deletions: BTreeMap<String, u32>,
    pub insertions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Match(Ph),
    Sub(Ph, Ph),
    /// Expected, but not heard.
    Del(Ph),
    /// Heard, but not expected.
    Ins(Ph),
}

/// Best alignment of what was `expected` against what was `heard`, by acoustic distance.
pub fn align(expected: &[Ph], heard: &[Ph]) -> Vec<Edit> {
    let (n, m) = (expected.len(), heard.len());
    let mut d = vec![vec![0.0f64; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i as f64 * INDEL_ALIGN_COST;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j as f64 * INDEL_ALIGN_COST;
    }
    for i in 1..=n {
        for j in 1..=m {
            d[i][j] = (d[i - 1][j - 1] + phoneme::distance(expected[i - 1], heard[j - 1]))
                .min(d[i - 1][j] + INDEL_ALIGN_COST)
                .min(d[i][j - 1] + INDEL_ALIGN_COST);
        }
    }
    let (mut i, mut j) = (n, m);
    let mut edits = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = phoneme::distance(expected[i - 1], heard[j - 1]);
            if (d[i][j] - (d[i - 1][j - 1] + cost)).abs() < 1e-9 {
                edits.push(if cost == 0.0 {
                    Edit::Match(expected[i - 1])
                } else {
                    Edit::Sub(expected[i - 1], heard[j - 1])
                });
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && (d[i][j] - (d[i - 1][j] + INDEL_ALIGN_COST)).abs() < 1e-9 {
            edits.push(Edit::Del(expected[i - 1]));
            i -= 1;
        } else {
            edits.push(Edit::Ins(heard[j - 1]));
            j -= 1;
        }
    }
    edits.reverse();
    edits
}

impl Stats {
    pub fn record(&mut self, edits: &[Edit]) {
        for edit in edits {
            match *edit {
                Edit::Match(e) => *self.opportunities.entry(phoneme::name(e).into()).or_default() += 1,
                Edit::Sub(e, h) => {
                    *self.opportunities.entry(phoneme::name(e).into()).or_default() += 1;
                    *self
                        .substitutions
                        .entry(format!("{}>{}", phoneme::name(e), phoneme::name(h)))
                        .or_default() += 1;
                }
                Edit::Del(e) => {
                    *self.opportunities.entry(phoneme::name(e).into()).or_default() += 1;
                    *self.deletions.entry(phoneme::name(e).into()).or_default() += 1;
                }
                Edit::Ins(_) => self.insertions += 1,
            }
        }
    }

    pub fn opportunities_of(&self, p: Ph) -> u32 {
        self.opportunities.get(phoneme::name(p)).copied().unwrap_or(0)
    }

    pub fn substitution_count(&self, e: Ph, h: Ph) -> u32 {
        self.substitutions
            .get(&format!("{}>{}", phoneme::name(e), phoneme::name(h)))
            .copied()
            .unwrap_or(0)
    }

    pub fn substitutions_of(&self, e: Ph) -> u32 {
        (0..COUNT as Ph).map(|h| self.substitution_count(e, h)).sum()
    }
}

/// Probabilities derived from [`Stats`].
#[derive(Debug, Clone)]
pub struct Model {
    /// `p[e][h]`: chance that expected phoneme `e` is heard as `h` (`h == e` is a clean hit).
    p: Vec<[f64; COUNT]>,
    del: [f64; COUNT],
    ins: f64,
}

impl Model {
    pub fn from_stats(stats: &Stats) -> Self {
        let total_opps: u32 = stats.opportunities.values().sum();
        let mut p = vec![[0.0; COUNT]; COUNT];
        let mut del = [0.0; COUNT];

        for e in 0..COUNT as Ph {
            let opps = f64::from(stats.opportunities_of(e));
            let subs = f64::from(stats.substitutions_of(e));
            let dels = f64::from(stats.deletions.get(phoneme::name(e)).copied().unwrap_or(0));
            let confusion = (subs + BASE_CONFUSION * PRIOR_WEIGHT) / (opps + PRIOR_WEIGHT);
            let deletion = (dels + BASE_DELETION * PRIOR_WEIGHT) / (opps + PRIOR_WEIGHT);
            del[e as usize] = deletion;

            // Which sound it turns into: acoustic neighbours by default, the
            // user's own history once there is some.
            let near = |h: Ph| if h != e && phoneme::distance(e, h) < 3.0 { (-2.0 * phoneme::distance(e, h)).exp() } else { 0.0 };
            let near_total: f64 = (0..COUNT as Ph).map(near).sum();
            for h in 0..COUNT as Ph {
                if h == e {
                    continue;
                }
                let prior_share = if near_total > 0.0 { near(h) / near_total } else { 0.0 };
                let seen = f64::from(stats.substitution_count(e, h));
                p[e as usize][h as usize] = confusion * (seen + SHARE_WEIGHT * prior_share) / (subs + SHARE_WEIGHT);
            }
            p[e as usize][e as usize] = (1.0 - confusion - deletion).max(0.01);
        }

        let ins = (f64::from(stats.insertions) + BASE_INSERTION * PRIOR_WEIGHT)
            / (f64::from(total_opps) + PRIOR_WEIGHT)
            / COUNT as f64;
        Self { p, del, ins }
    }

    /// Chance expected `e` is heard as `h`.
    pub fn p(&self, e: Ph, h: Ph) -> f64 {
        self.p[e as usize][h as usize]
    }

    /// Log-likelihood of the recognizer producing `heard` when the user meant `meant`,
    /// over the best alignment of the two.
    pub fn ln_likelihood(&self, meant: &[Ph], heard: &[Ph]) -> f64 {
        let (n, m) = (meant.len(), heard.len());
        let mut f = vec![vec![f64::NEG_INFINITY; m + 1]; n + 1];
        f[0][0] = 0.0;
        for i in 0..=n {
            for j in 0..=m {
                if i == 0 && j == 0 {
                    continue;
                }
                let mut best = f64::NEG_INFINITY;
                if i > 0 && j > 0 {
                    best = best.max(f[i - 1][j - 1] + self.p(meant[i - 1], heard[j - 1]).ln());
                }
                if i > 0 {
                    best = best.max(f[i - 1][j] + self.del[meant[i - 1] as usize].ln());
                }
                if j > 0 {
                    best = best.max(f[i][j - 1] + self.ins.ln());
                }
                f[i][j] = best;
            }
        }
        f[n][m]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(s: &str) -> Vec<Ph> {
        s.split_whitespace().map(|p| phoneme::parse(p).unwrap()).collect()
    }

    #[test]
    fn aligns_a_single_substitution() {
        let edits = align(&seq("TH IH NG K"), &seq("F IH NG K"));
        let th = phoneme::parse("TH").unwrap();
        let f = phoneme::parse("F").unwrap();
        assert_eq!(edits[0], Edit::Sub(th, f));
        assert!(edits[1..].iter().all(|e| matches!(e, Edit::Match(_))));
    }

    #[test]
    fn aligns_a_dropped_sound() {
        let edits = align(&seq("K AE T S"), &seq("K AE T"));
        assert!(matches!(edits.last(), Some(Edit::Del(_))));
    }

    #[test]
    fn untrained_model_prefers_hits_and_near_misses() {
        let m = Model::from_stats(&Stats::default());
        let th = phoneme::parse("TH").unwrap();
        let (f, k) = (phoneme::parse("F").unwrap(), phoneme::parse("K").unwrap());
        assert!(m.p(th, th) > 0.9);
        assert!(m.p(th, f) > m.p(th, k), "th→f is a closer sound than th→k");
        // Each row is a probability distribution (hits + confusions + deletion ≈ 1).
        let row: f64 = (0..COUNT as Ph).map(|h| m.p(th, h)).sum::<f64>() + m.del[th as usize];
        assert!((row - 1.0).abs() < 0.02, "{row}");
    }

    #[test]
    fn evidence_raises_the_learned_confusion() {
        let (th, f) = (phoneme::parse("TH").unwrap(), phoneme::parse("F").unwrap());
        let before = Model::from_stats(&Stats::default()).p(th, f);
        let mut stats = Stats::default();
        for _ in 0..12 {
            stats.record(&[Edit::Sub(th, f)]);
        }
        for _ in 0..8 {
            stats.record(&[Edit::Match(th)]);
        }
        let after = Model::from_stats(&stats).p(th, f);
        assert!(after > before * 20.0, "{before} → {after}");
        assert!(after < 0.6, "twelve samples don't make it a certainty");
    }

    #[test]
    fn likelihood_ranks_the_matching_pronunciation_higher() {
        let m = Model::from_stats(&Stats::default());
        let heard = seq("F IH NG K");
        let same = m.ln_likelihood(&seq("F IH NG K"), &heard);
        let near = m.ln_likelihood(&seq("TH IH NG K"), &heard);
        let far = m.ln_likelihood(&seq("K IH NG K"), &heard);
        assert!(same > near && near > far, "{same} {near} {far}");
    }
}
