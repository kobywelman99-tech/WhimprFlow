# Data

Both files are embedded in the binary (desktop and iOS).

## `cmudict.dict`: pronunciations
The CMU Pronouncing Dictionary (Carnegie Mellon University). BSD-style licence in
`CMUDICT-LICENSE`; its binary-redistribution clause means the shipped app must reproduce that
notice (see "Open: acknowledgements" in `tasks/backend-gaps.md`).

- Source: https://github.com/cmusphinx/cmudict, `cmudict.dict`
- SHA-256: `81917843c7f44ce2b094ac63873c2c7a4cf802040792c455ba3ca406891c3d22`

## `wordfreq.txt`: how common each word is
The 40,000 most frequent words and their counts (word, space, count), taken from Peter Norvig's
`count_1w.txt` (https://norvig.com/ngrams/).

- SHA-256: `73ed7fbf9be9f5aad38e35717eaefac8177ba6eb59f3f928f413036afa5e9a3e`
- **Licence status: unresolved.** Norvig's page puts his *code* under the MIT licence. It describes
  the data files only as "derived from the Google Web Trillion Word Corpus… distributed by the
  Linguistic Data Consortium" and states no licence for them. Word counts are facts, but until
  this is settled the file should not ship in a release. Candidates to replace it: counts computed
  from a CC0 or public-domain corpus (for example Common Voice's CC0 sentences, which also match
  dictation better than book text), or a source with a clear attribution-only licence.
