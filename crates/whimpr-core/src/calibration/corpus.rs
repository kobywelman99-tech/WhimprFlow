//! Bundled calibration prompts. Public-domain literature and speeches, plus
//! original sentences built to cover a wide spread of English sounds.
//!
//! Each prompt has a `tag` naming the tradition it draws on, so a session can mix
//! them: sounds are stressed differently by a poet, an orator and a technical writer.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub id: u32,
    pub tag: &'static str,
    pub text: &'static str,
}

const fn p(id: u32, tag: &'static str, text: &'static str) -> Prompt {
    Prompt { id, tag, text }
}

pub const ALL: &[Prompt] = &[
    // Narrative prose
    p(1, "prose", "It was the best of times, it was the worst of times, it was the age of wisdom, it was the age of foolishness."),
    p(2, "prose", "It is a truth universally acknowledged, that a single man in possession of a good fortune, must be in want of a wife."),
    p(3, "prose", "Call me Ishmael. Some years ago, never mind how long precisely, I thought I would sail about a little."),
    p(4, "prose", "Curiouser and curiouser, cried Alice, she was so much surprised that for the moment she quite forgot how to speak good English."),
    p(5, "prose", "The cold passed reluctantly from the earth, and the retiring fogs revealed an army stretched out on the hills, resting."),
    p(6, "prose", "You don't know about me without you have read a book by the name of The Adventures of Tom Sawyer, but that ain't no matter."),
    // Poetry and verse
    p(7, "verse", "Shall I compare thee to a summer's day? Thou art more lovely and more temperate."),
    p(8, "verse", "Once upon a midnight dreary, while I pondered, weak and weary, over many a quaint and curious volume of forgotten lore."),
    p(9, "verse", "I wandered lonely as a cloud that floats on high over vales and hills."),
    p(10, "verse", "Two roads diverged in a yellow wood, and sorry I could not travel both and be one traveler, long I stood."),
    p(11, "verse", "Tyger Tyger, burning bright, in the forests of the night."),
    // Rhetoric and oratory
    p(12, "oratory", "Four score and seven years ago our fathers brought forth on this continent a new nation, conceived in liberty."),
    p(13, "oratory", "We hold these truths to be self-evident, that all men are created equal, that they are endowed by their Creator with certain unalienable rights."),
    p(14, "oratory", "I have nothing to offer but blood, toil, tears and sweat."),
    p(15, "oratory", "The only thing we have to fear is fear itself, nameless, unreasoning, unjustified terror."),
    // Wit and dialogue
    p(16, "wit", "I can resist everything except temptation."),
    p(17, "wit", "The report of my death was an exaggeration."),
    p(18, "wit", "To be, or not to be, that is the question."),
    p(19, "wit", "Elementary, my dear Watson, you see but you do not observe."),
    // Phonetic coverage (original sentences)
    p(20, "phonetic", "The quick brown fox jumps over the lazy dog while the thirteen thieves watched through the weathered window."),
    p(21, "phonetic", "She sells seashells by the seashore, and the shells she sells are surely seashells."),
    p(22, "phonetic", "A thorough thought about rural, familiar, and library schedules vexed the particular judge."),
    p(23, "phonetic", "Please ask the chief engineer whether the measure of pleasure is worth the treasure."),
    p(24, "phonetic", "Water, bottle, butter, better: the little letter went to the city at eight."),
    p(25, "phonetic", "Three free trees threw thick, thin, and through shadows across the three-thirty train."),
    // Minimal pairs, said as short phrases
    p(26, "minimal-pairs", "ship, sheep, chip, cheap, bit, beat, pull, pool, full, fool."),
    p(27, "minimal-pairs", "think, sink, thing, sing, then, den, vest, west, very, berry."),
    p(28, "minimal-pairs", "cat, cut, cot, caught, bat, bet, but, bought, pen, pin."),
    p(29, "minimal-pairs", "light, right, late, rate, lock, rock, collect, correct, glass, grass."),
    // Numbers, dates and units
    p(30, "numbers", "The meeting is on March thirteenth at a quarter past two, room four hundred and twelve."),
    p(31, "numbers", "It cost nineteen dollars and ninety-five cents, which is about fifteen percent more than last year."),
    p(32, "numbers", "Call me on 555 0134, or send it to extension 2210 before the end of the month."),
    // Everyday and conversational
    p(33, "everyday", "Could you remind me to pick up the kids at three, and also to call the dentist before Friday?"),
    p(34, "everyday", "I was going to say something, but honestly I think we should probably just wait and see what happens."),
    p(35, "everyday", "Let me know if that works for you, otherwise we can push it to next week."),
    // Technical and professional
    p(36, "technical", "The API returns a JSON payload, and the cache invalidates automatically when the database schema changes."),
    p(37, "technical", "Deploy the staging build, run the regression suite, and then promote it to production if everything passes."),
    p(38, "technical", "The patient presented with acute abdominal pain, nausea, and a mild fever lasting several hours."),
    // Names and unusual words (user adds their own in the app)
    p(39, "names", "Saoirse, Siobhan, Nguyen, Xavier, Ioannis, Aaliyah, Przemysl, and Worcestershire."),
    p(40, "names", "Quinoa, espresso, bruschetta, croissant, gnocchi, pho, and jalapeno are on the menu."),
];

/// Pick `count` prompts, round-robin across tags so a session spans every style,
/// with a seeded shuffle inside each tag so repeat sessions see different lines.
pub fn prompts(count: usize, seed: u64) -> Vec<Prompt> {
    let mut tags: Vec<&'static str> = Vec::new();
    for pr in ALL {
        if !tags.contains(&pr.tag) {
            tags.push(pr.tag);
        }
    }
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let mut buckets: Vec<Vec<Prompt>> = tags
        .iter()
        .map(|t| ALL.iter().filter(|p| p.tag == *t).cloned().collect())
        .collect();
    for b in &mut buckets {
        for i in (1..b.len()).rev() {
            b.swap(i, (next() % (i as u64 + 1)) as usize);
        }
    }

    let mut out = Vec::new();
    let mut round = 0;
    while out.len() < count {
        let mut progressed = false;
        for b in &buckets {
            if let Some(pr) = b.get(round) {
                out.push(pr.clone());
                progressed = true;
                if out.len() == count {
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
        round += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = ALL.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn selection_spans_tags_and_is_deterministic() {
        let a = prompts(10, 7);
        assert_eq!(a, prompts(10, 7));
        let tags: std::collections::HashSet<_> = a.iter().map(|p| p.tag).collect();
        assert_eq!(tags.len(), 10);
        assert_ne!(prompts(10, 7), prompts(10, 8));
    }

    #[test]
    fn count_beyond_corpus_is_capped() {
        assert_eq!(prompts(1000, 1).len(), ALL.len());
    }
}
