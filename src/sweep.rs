// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Text sweeps: find every term on a watch list in any number of text files, one pass per file.
//!
//! The terms are compiled once into an Aho-Corasick machine (`skeletrace_algo::textsearch`), so each file is
//! read exactly once however long the watch list is. Files are read in chunks (1 MiB by default), so a dump
//! never has to fit in memory, and a term split across two chunks is still found.
//!
//! Every hit records its file, byte offset, line, column (counted in characters, as editors show them) and
//! the text exactly as written there. A UTF-8 byte-order mark at the start of a file is hidden by editors, so it
//! doesn't count towards line 1's columns. Per-term counts and the list of terms never found are always complete;
//! the list of individual hits stops at a cap, and the report says when it did.
//!
//! Files are searched as bytes, so UTF-8 and plain ASCII text work as they are. A file that starts with a
//! UTF-16 byte-order mark gets a warning in its summary, because UTF-8 terms can't match UTF-16 text.
//!
//! ## Near-misses
//!
//! With fuzzy matching on ([`Sweeper::with_fuzzy`]), the same pass also splits the text into words and lists
//! every word a typo or two away from a term: "jdeo88" for "jdoe88". A word is a run of letters and digits in
//! any alphabet and `@ . _ - + #`, with full stops trimmed from both ends, so the "jdeo88." that ends a sentence
//! is the word "jdeo88". A word holding an `@` is also tried piece by piece, so the handle in
//! "jdeo88@example.net" is found as well. Typos are counted as in `skeletrace_algo::fuzzy::osa_distance`: an
//! insertion, a deletion, a substitution, or a swap of two neighbouring characters. The words are checked
//! against all the terms at once with `skeletrace_algo::fuzzy::NearSet`.
//!
//! How many typos a term may have depends on its length ([`edits_allowed`]): none below five characters, where
//! too many ordinary words sit one typo away, one up to seven, and two from eight. Terms that aren't one word,
//! such as "+44 7700 900123", are only matched exactly, and the report lists them with the reason. A word that
//! holds a term exactly is left to the exact sweep, so nothing is reported twice. Near-misses come back in their
//! own list, with their own counts, so the exact results read just as they do without fuzzy matching.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use skeletrace_algo::fuzzy::{NameMatch, NearSearcher, NearSet, NearSetError};
use skeletrace_algo::textsearch::{Hit, PatternSet, PatternSetError};

/// How many hits a report lists by default. Counts are never capped.
pub const DEFAULT_MAX_HITS: usize = 10_000;

/// How many bytes are read at a time by default: 1 MiB.
pub const DEFAULT_CHUNK_BYTES: usize = 1 << 20;

/// The most typos fuzzy matching can allow.
pub const MAX_FUZZY_EDITS: usize = 2;

/// Terms shorter than this many characters are only matched exactly: too many ordinary words sit one typo away
/// from a short term ("moth" from "math", "both" and "mote").
pub const NEAR_MIN_CHARS: usize = 5;

/// Terms at least this many characters long may be two typos away; shorter ones, one.
pub const NEAR_TWO_EDIT_CHARS: usize = 8;

/// The longest run of word characters taken as a word, in bytes, unless a term is longer. Longer runs, such as
/// base64 blocks, are skipped instead of being held in memory.
pub const NEAR_WORD_BYTES: usize = 4096;

/// The reason given for a term that can't be matched fuzzily because it isn't one word.
const NOT_ONE_WORD: &str =
    "not one word: words hold only letters, digits and @ . _ - + #, and don't start or end with a full stop";

fn default_max_hits() -> usize {
    DEFAULT_MAX_HITS
}

fn default_chunk_bytes() -> usize {
    DEFAULT_CHUNK_BYTES
}

/// Why a sweep couldn't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepError {
    /// The watch list has no terms once blank lines and notes are left out.
    NoTerms,
    /// No files were given to search.
    NoInputs,
    /// A term contains a line break. Terms are matched within a line.
    TermSpansLines(String),
    /// The watch list isn't UTF-8 or UTF-16 text.
    BadWatchList(String),
    /// A setting is out of range.
    BadSetting(String),
    /// The terms add up to more than the search machine can index.
    TooLarge,
    /// A file couldn't be read.
    Io {
        /// The file.
        path: String,
        /// What went wrong.
        message: String,
    },
}

impl fmt::Display for SweepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTerms => write!(f, "the watch list has no terms"),
            Self::NoInputs => write!(f, "no files to search"),
            Self::TermSpansLines(term) => write!(f, "the term {term:?} contains a line break"),
            Self::BadWatchList(why) => write!(f, "cannot read the watch list: {why}"),
            Self::BadSetting(why) => write!(f, "{why}"),
            Self::TooLarge => write!(f, "the watch list is too large to index"),
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
        }
    }
}

impl std::error::Error for SweepError {}

impl From<PatternSetError> for SweepError {
    fn from(_: PatternSetError) -> Self {
        Self::TooLarge
    }
}

impl From<NearSetError> for SweepError {
    fn from(err: NearSetError) -> Self {
        match err {
            NearSetError::TooLarge => Self::TooLarge,
            other => Self::BadSetting(other.to_string()),
        }
    }
}

/// How many typos a term of `chars` characters may have when fuzzy matching allows up to `fuzzy`: none below
/// [`NEAR_MIN_CHARS`], one below [`NEAR_TWO_EDIT_CHARS`], two from there, and never more than `fuzzy`.
pub fn edits_allowed(chars: usize, fuzzy: usize) -> usize {
    let by_length = if chars >= NEAR_TWO_EDIT_CHARS {
        2
    } else if chars >= NEAR_MIN_CHARS {
        1
    } else {
        0
    };
    by_length.min(fuzzy)
}

/// Whether `c` belongs inside a word when looking for near-misses: a letter or digit in any alphabet, or one of
/// `@ . _ - + #`.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+' | '#')
}

/// Whether `term` is one whole word, as the text is split for near-misses.
fn is_one_word(term: &str) -> bool {
    !term.is_empty()
        && term.chars().all(is_word_char)
        && !term.starts_with('.')
        && !term.ends_with('.')
}

/// What to sweep: the terms, the files, and how to match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepRequest {
    /// The terms to look for. Blank terms are ignored, and repeats are searched once.
    pub terms: Vec<String>,
    /// The files to search, in order.
    pub inputs: Vec<PathBuf>,
    /// When true, letters must match exactly. When false (the default), ASCII capitals are ignored, so
    /// "Lantern@Example.org" finds "lantern@example.ORG".
    #[serde(default)]
    pub exact: bool,
    /// The most typos a near-miss may have: 1 or 2, or 0 (the default) for exact matches only. Each term's own
    /// allowance also depends on its length; see [`edits_allowed`].
    #[serde(default)]
    pub fuzzy: usize,
    /// The most hits to list, and separately the most near-misses. Counts stay complete beyond it.
    #[serde(default = "default_max_hits")]
    pub max_hits: usize,
    /// Bytes read at a time.
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: usize,
}

impl SweepRequest {
    /// A request with the default settings: capitals ignored, no near-misses, 10,000 hits listed, 1 MiB chunks.
    pub fn new(terms: Vec<String>, inputs: Vec<PathBuf>) -> Self {
        Self {
            terms,
            inputs,
            exact: false,
            fuzzy: 0,
            max_hits: DEFAULT_MAX_HITS,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
        }
    }
}

/// One file's part in a sweep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepInputSummary {
    /// The file, as given.
    pub path: String,
    /// How many bytes were read.
    pub bytes: u64,
    /// How many hits it held.
    pub hits: u64,
    /// How many near-misses it held.
    #[serde(default)]
    pub near_hits: u64,
    /// Anything that may have stopped terms being found, such as UTF-16 text.
    pub warning: Option<String>,
}

/// How often one term was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepTermCount {
    /// The term, as given in the watch list.
    pub term: String,
    /// Hits across every file.
    pub hits: u64,
    /// How many files held at least one.
    pub files: usize,
    /// Near-misses across every file.
    #[serde(default)]
    pub near_hits: u64,
    /// How many typos a near-miss of this term may have: 0 when it is only matched exactly.
    #[serde(default)]
    pub edits_allowed: usize,
}

/// One place a term was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepHit {
    /// The term, as given in the watch list.
    pub term: String,
    /// The file it was found in.
    pub input: String,
    /// Byte offset of the match from the start of the file.
    pub offset: u64,
    /// Line number, from 1.
    pub line: u64,
    /// Column, from 1, counted in characters (UTF-8), as text editors count them.
    pub column: u64,
    /// The text exactly as written in the file.
    pub matched: String,
}

/// One word found a typo or two away from a term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepNearHit {
    /// The term, as given in the watch list.
    pub term: String,
    /// The file it was found in.
    pub input: String,
    /// Byte offset of the word, or of the piece of it that matched, from the start of the file.
    pub offset: u64,
    /// Line number, from 1.
    pub line: u64,
    /// Column, from 1, counted in characters, as for hits.
    pub column: u64,
    /// The word, or the piece of it between `@` signs, exactly as written in the file.
    pub matched: String,
    /// How many typos apart the word and the term are: insertions, deletions, substitutions, and swaps of two
    /// neighbouring characters.
    pub edits: usize,
}

/// A term that is only matched exactly, though fuzzy matching was asked for, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepNearSkip {
    /// The term, as given in the watch list.
    pub term: String,
    /// Why it gets no near-misses.
    pub reason: String,
}

/// The result of a sweep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepReport {
    /// Whether letters had to match exactly.
    pub exact: bool,
    /// The most typos a near-miss could have: 0 when fuzzy matching was off.
    #[serde(default)]
    pub fuzzy: usize,
    /// How many distinct terms were searched for.
    pub terms: usize,
    /// One summary per file, in the order searched.
    pub inputs: Vec<SweepInputSummary>,
    /// One count per term, in watch-list order.
    pub counts: Vec<SweepTermCount>,
    /// The terms found nowhere as written, in watch-list order.
    pub missing: Vec<String>,
    /// Every hit, counted.
    pub hits_total: u64,
    /// The hits, by file and then by where they end, up to the cap.
    pub hits: Vec<SweepHit>,
    /// True when more hits were found than listed.
    pub truncated: bool,
    /// Every near-miss, counted.
    #[serde(default)]
    pub near_hits_total: u64,
    /// The near-misses, by file, then by where they start, then closest first, then in watch-list order, up to
    /// the cap.
    #[serde(default)]
    pub near_hits: Vec<SweepNearHit>,
    /// True when more near-misses were found than listed.
    #[serde(default)]
    pub near_truncated: bool,
    /// The terms with no exact hit but at least one near-miss, in watch-list order.
    #[serde(default)]
    pub near_only: Vec<String>,
    /// The terms only matched exactly although fuzzy matching was on, with the reason, in watch-list order.
    #[serde(default)]
    pub near_skipped: Vec<SweepNearSkip>,
}

/// Reads a watch list: one term per line, in UTF-8 (with or without a byte-order mark) or UTF-16 (as
/// Windows PowerShell writes it). Spaces around each term are trimmed. Blank lines are skipped, and so are
/// notes: lines that are just `#`, or start with `#` and a space. A term such as `#lantern` is kept.
pub fn parse_terms(bytes: &[u8]) -> Result<Vec<String>, SweepError> {
    let text = decode_text(bytes)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "#" && !line.starts_with("# "))
        .map(str::to_string)
        .collect())
}

fn decode_text(bytes: &[u8]) -> Result<String, SweepError> {
    let utf16 = |little: bool| -> Result<String, SweepError> {
        let pairs = bytes[2..].chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return Err(SweepError::BadWatchList(
                "UTF-16 text with an odd number of bytes".into(),
            ));
        }
        let units: Vec<u16> = pairs
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|err| SweepError::BadWatchList(err.to_string()))
    };
    match bytes {
        [0xFF, 0xFE, ..] => utf16(true),
        [0xFE, 0xFF, ..] => utf16(false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec())
            .map_err(|err| SweepError::BadWatchList(err.to_string())),
        _ => String::from_utf8(bytes.to_vec())
            .map_err(|err| SweepError::BadWatchList(err.to_string())),
    }
}

/// The terms that get near-misses, compiled for checking words against all of them at once.
#[derive(Debug, Clone)]
struct NearTerms {
    set: NearSet,
    /// For each term in the set: its place in the watch list, and its text as compared (capitals folded unless
    /// matching is exact).
    terms: Vec<(usize, String)>,
    /// The longest run of word characters kept as a word, in bytes.
    word_cap: usize,
}

/// A sweep in progress: feed it files or readers, then [`Sweeper::finish`] for the report.
#[derive(Debug, Clone)]
pub struct Sweeper {
    set: PatternSet,
    terms: Vec<String>,
    exact: bool,
    max_hits: usize,
    chunk_bytes: usize,
    longest: usize,
    counts: Vec<u64>,
    files: Vec<usize>,
    inputs: Vec<SweepInputSummary>,
    hits: Vec<SweepHit>,
    hits_total: u64,
    fuzzy: usize,
    near: Option<NearTerms>,
    allowed: Vec<usize>,
    near_skipped: Vec<SweepNearSkip>,
    near_counts: Vec<u64>,
    near_hits: Vec<SweepNearHit>,
    near_total: u64,
}

impl Sweeper {
    /// Compiles `terms`. Blank terms are dropped, and repeats (as matching would see them) are searched once,
    /// keeping the first spelling.
    pub fn new(terms: &[String], exact: bool, max_hits: usize) -> Result<Self, SweepError> {
        let mut kept: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for term in terms {
            if term.is_empty() {
                continue;
            }
            if term.contains(['\n', '\r']) {
                return Err(SweepError::TermSpansLines(term.clone()));
            }
            let key = if exact {
                term.clone()
            } else {
                term.to_ascii_lowercase()
            };
            if seen.insert(key) {
                kept.push(term.clone());
            }
        }
        if kept.is_empty() {
            return Err(SweepError::NoTerms);
        }
        let set = if exact {
            PatternSet::new(&kept)?
        } else {
            PatternSet::new_ascii_case_insensitive(&kept)?
        };
        let longest = kept.iter().map(String::len).max().unwrap_or(0);
        let n = kept.len();
        Ok(Self {
            set,
            terms: kept,
            exact,
            max_hits,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            longest,
            counts: vec![0; n],
            files: vec![0; n],
            inputs: Vec::new(),
            hits: Vec::new(),
            hits_total: 0,
            fuzzy: 0,
            near: None,
            allowed: vec![0; n],
            near_skipped: Vec::new(),
            near_counts: vec![0; n],
            near_hits: Vec::new(),
            near_total: 0,
        })
    }

    /// Reads `bytes` at a time instead of 1 MiB. At least 1.
    pub fn with_chunk_bytes(mut self, bytes: usize) -> Result<Self, SweepError> {
        if bytes == 0 {
            return Err(SweepError::BadSetting(
                "chunk size must be at least 1 byte".into(),
            ));
        }
        self.chunk_bytes = bytes;
        Ok(self)
    }

    /// Also lists near-misses: words up to `fuzzy` typos from a term, 1 or 2. 0 turns them off. Each term's own
    /// allowance depends on its length ([`edits_allowed`]), and terms that aren't one word are only matched
    /// exactly. Call it before sweeping anything.
    pub fn with_fuzzy(mut self, fuzzy: usize) -> Result<Self, SweepError> {
        if fuzzy > MAX_FUZZY_EDITS {
            return Err(SweepError::BadSetting(format!(
                "fuzzy matching allows 1 or 2 typos, not {fuzzy}"
            )));
        }
        self.fuzzy = fuzzy;
        self.near = None;
        self.allowed = vec![0; self.terms.len()];
        self.near_skipped.clear();
        if fuzzy == 0 {
            return Ok(self);
        }
        let mut picked: Vec<(usize, String, usize)> = Vec::new();
        for (index, term) in self.terms.iter().enumerate() {
            let allowed = edits_allowed(term.chars().count(), fuzzy);
            let reason = if !is_one_word(term) {
                Some(NOT_ONE_WORD.to_string())
            } else if allowed == 0 {
                Some(format!("shorter than {NEAR_MIN_CHARS} characters"))
            } else {
                None
            };
            match reason {
                Some(reason) => self.near_skipped.push(SweepNearSkip {
                    term: term.clone(),
                    reason,
                }),
                None => {
                    self.allowed[index] = allowed;
                    let compared = if self.exact {
                        term.clone()
                    } else {
                        term.to_ascii_lowercase()
                    };
                    picked.push((index, compared, allowed));
                }
            }
        }
        if picked.is_empty() {
            return Ok(self);
        }
        let set = NearSet::new(
            picked
                .iter()
                .map(|(_, compared, allowed)| (compared.as_str(), *allowed)),
        )?;
        let longest = picked
            .iter()
            .map(|(_, compared, _)| compared.len())
            .max()
            .unwrap_or(0);
        self.near = Some(NearTerms {
            set,
            terms: picked
                .into_iter()
                .map(|(index, compared, _)| (index, compared))
                .collect(),
            // A word within two typos of the longest term is at most a few bytes longer than it.
            word_cap: NEAR_WORD_BYTES.max(longest + 64),
        });
        Ok(self)
    }

    /// Searches one file.
    pub fn sweep_file(&mut self, path: &Path) -> Result<(), SweepError> {
        let name = path.display().to_string();
        let file = File::open(path).map_err(|err| SweepError::Io {
            path: name.clone(),
            message: err.to_string(),
        })?;
        self.sweep_reader(&name, file)
    }

    /// Searches everything `reader` gives, reporting it under `name`.
    pub fn sweep_reader<R: Read>(&mut self, name: &str, mut reader: R) -> Result<(), SweepError> {
        let io_error = |err: io::Error| SweepError::Io {
            path: name.to_string(),
            message: err.to_string(),
        };
        let mut scanner = self.set.scanner();
        let mut buffer = vec![0u8; self.chunk_bytes];
        let mut found: Vec<Hit> = Vec::new();
        let mut in_this_file = vec![false; self.terms.len()];
        // The end of the text before this chunk, long enough to rebuild a match that started there.
        let keep = self.longest.saturating_sub(1);
        let mut tail: Vec<u8> = Vec::with_capacity(keep);
        let mut cols = Columns::new();
        let mut read_so_far = 0usize;
        let mut file_hits = 0u64;
        let mut head: Vec<u8> = Vec::with_capacity(2); // the first two bytes, for spotting UTF-16

        // Near-misses, when some term gets them: the same chunks are split into words and each word is checked.
        let mut near = self.near.as_ref().map(|terms| {
            (
                WordReader::new(terms.word_cap),
                NearPass {
                    terms,
                    searcher: terms.set.searcher(),
                    names: &self.terms,
                    exact: self.exact,
                    max_hits: self.max_hits,
                    input: name,
                    hits: &mut self.near_hits,
                    counts: &mut self.near_counts,
                    total: &mut self.near_total,
                    file_hits: 0,
                    matches: Vec::new(),
                    best: Vec::new(),
                    pieces: Vec::new(),
                    folded: String::new(),
                },
            )
        });
        loop {
            let n = loop {
                match reader.read(&mut buffer) {
                    Ok(n) => break n,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(io_error(err)),
                }
            };
            if n == 0 {
                break;
            }
            let chunk = &buffer[..n];
            if head.len() < 2 {
                let wanted = (2 - head.len()).min(n);
                head.extend_from_slice(&chunk[..wanted]);
            }
            scanner.feed(chunk, &mut found);
            // Lines and columns only matter while hits are still being listed.
            let mut cursor = 0usize;
            for hit in found.drain(..) {
                self.hits_total += 1;
                file_hits += 1;
                self.counts[hit.pattern] += 1;
                in_this_file[hit.pattern] = true;
                if self.hits.len() >= self.max_hits {
                    continue;
                }
                let end_here = hit.end - read_so_far;
                for &byte in &chunk[cursor..end_here] {
                    cols.advance(byte);
                }
                cursor = end_here;
                let matched = rebuild(&tail, chunk, read_so_far, hit.start, hit.end);
                let match_chars = matched.iter().filter(|&&b| b & 0xC0 != 0x80).count() as u64;
                self.hits.push(SweepHit {
                    term: self.terms[hit.pattern].clone(),
                    input: name.to_string(),
                    offset: hit.start as u64,
                    line: cols.line,
                    column: cols.column.saturating_sub(match_chars) + 1,
                    matched: String::from_utf8_lossy(&matched).into_owned(),
                });
            }
            if self.hits.len() < self.max_hits {
                for &byte in &chunk[cursor..] {
                    cols.advance(byte);
                }
            }
            if let Some((words, pass)) = near.as_mut() {
                words.feed(chunk, read_so_far as u64, &mut |word: &Word<'_>| {
                    pass.word(word)
                });
            }
            if n >= keep {
                tail.clear();
                tail.extend_from_slice(&chunk[n - keep..]);
            } else {
                tail.extend_from_slice(chunk);
                let excess = tail.len().saturating_sub(keep);
                tail.drain(..excess);
            }
            read_so_far += n;
        }
        let near_file_hits = match near.as_mut() {
            Some((words, pass)) => {
                words.finish(&mut |word: &Word<'_>| pass.word(word));
                pass.file_hits
            }
            None => 0,
        };
        for (term, &hit) in in_this_file.iter().enumerate() {
            if hit {
                self.files[term] += 1;
            }
        }
        let warning = matches!(head.as_slice(), [0xFF, 0xFE] | [0xFE, 0xFF]).then(|| {
            "this file is UTF-16 text, so UTF-8 terms can't match it: save it as UTF-8 first"
                .to_string()
        });
        self.inputs.push(SweepInputSummary {
            path: name.to_string(),
            bytes: read_so_far as u64,
            hits: file_hits,
            near_hits: near_file_hits,
            warning,
        });
        Ok(())
    }

    /// The report for everything searched so far.
    pub fn finish(self) -> SweepReport {
        let counts: Vec<SweepTermCount> = self
            .terms
            .iter()
            .enumerate()
            .map(|(i, term)| SweepTermCount {
                term: term.clone(),
                hits: self.counts[i],
                files: self.files[i],
                near_hits: self.near_counts[i],
                edits_allowed: self.allowed[i],
            })
            .collect();
        let missing = counts
            .iter()
            .filter(|count| count.hits == 0)
            .map(|count| count.term.clone())
            .collect();
        let near_only = counts
            .iter()
            .filter(|count| count.hits == 0 && count.near_hits > 0)
            .map(|count| count.term.clone())
            .collect();
        SweepReport {
            exact: self.exact,
            fuzzy: self.fuzzy,
            terms: self.terms.len(),
            inputs: self.inputs,
            counts,
            missing,
            truncated: self.hits_total > self.hits.len() as u64,
            hits_total: self.hits_total,
            hits: self.hits,
            near_truncated: self.near_total > self.near_hits.len() as u64,
            near_hits_total: self.near_total,
            near_hits: self.near_hits,
            near_only,
            near_skipped: self.near_skipped,
        }
    }
}

/// Counts lines and columns as text editors show them. A line feed starts a new line, and every byte except a UTF-8
/// continuation byte (`10xxxxxx`) starts a character. A UTF-8 byte-order mark at the very start of a file is not a
/// character: editors hide it, and Windows PowerShell 5.1 writes one, so it doesn't count towards line 1's columns.
#[derive(Debug, Clone, Copy)]
struct Columns {
    /// The current line, from 1.
    line: u64,
    /// Characters read so far on this line.
    column: u64,
    /// How many of the file's first three bytes have been seen.
    head: u8,
    /// Whether those bytes so far are the start of a byte-order mark (`EF BB BF`).
    maybe_bom: bool,
}

impl Columns {
    fn new() -> Self {
        Self {
            line: 1,
            column: 0,
            head: 0,
            maybe_bom: true,
        }
    }

    /// Moves past one byte. Bytes must arrive in order, from the start of the file.
    fn advance(&mut self, byte: u8) {
        let mut bom_complete = false;
        if self.head < 3 {
            self.maybe_bom = self.maybe_bom && byte == [0xEF, 0xBB, 0xBF][usize::from(self.head)];
            self.head += 1;
            bom_complete = self.head == 3 && self.maybe_bom;
        }
        if byte == b'\n' {
            self.line += 1;
            self.column = 0;
        } else if byte & 0xC0 != 0x80 {
            self.column += 1;
        }
        if bom_complete {
            // The mark's first byte counted as a character; the mark isn't one.
            self.column -= 1;
        }
    }
}

/// The bytes `start..end` of the whole text, from this chunk and the tail of the text before it.
fn rebuild(tail: &[u8], chunk: &[u8], chunk_start: usize, start: usize, end: usize) -> Vec<u8> {
    let end_here = end - chunk_start;
    if start >= chunk_start {
        return chunk[start - chunk_start..end_here].to_vec();
    }
    let before = chunk_start - start;
    let mut bytes = tail[tail.len() - before..].to_vec();
    bytes.extend_from_slice(&chunk[..end_here]);
    bytes
}

/// Where a character starts: its byte offset, its line, and its column (from 1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Place {
    offset: u64,
    line: u64,
    column: u64,
}

/// A word split out of the text, and where it starts.
#[derive(Debug)]
struct Word<'a> {
    text: &'a str,
    at: Place,
}

// What each byte means to the word reader.
/// Ends a word: ASCII other than the word characters, and bytes UTF-8 never uses.
const SPLITS: u8 = 0;
/// An ASCII letter or digit, or one of `@ _ - + #`.
const WORD_BYTE: u8 = 1;
/// A full stop: inside a word, or trimmed from its ends.
const DOT: u8 = 2;
/// The first byte of a two-, three- or four-byte character.
const LEAD_2: u8 = 3;
const LEAD_3: u8 = 4;
const LEAD_4: u8 = 5;
/// A continuation byte, `10xxxxxx`.
const CONTINUES: u8 = 6;

/// The class of every byte, worked out once, at compile time.
const BYTE_CLASS: [u8; 256] = {
    let mut class = [SPLITS; 256];
    let mut i = 0;
    while i < 256 {
        class[i] = match i as u8 {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'@' | b'_' | b'-' | b'+' | b'#' => WORD_BYTE,
            b'.' => DOT,
            0x80..=0xBF => CONTINUES,
            0xC2..=0xDF => LEAD_2,
            0xE0..=0xEF => LEAD_3,
            0xF0..=0xF4 => LEAD_4,
            _ => SPLITS,
        };
        i += 1;
    }
    class
};

/// Splits a stream of bytes into words, a chunk at a time, for near-miss matching. A word cut by a chunk boundary
/// is kept whole, and so is a character. Bytes that aren't UTF-8 split words, as spaces do. Lines and columns are
/// counted by the same rule as hits.
#[derive(Debug)]
struct WordReader {
    /// The word so far, as written. Only whole characters go in, so it is always UTF-8.
    word: Vec<u8>,
    /// Where it starts.
    start: Place,
    /// Full stops just read: part of the word only if another word character follows.
    dots: usize,
    /// True once the word outgrew `cap` bytes: it is skipped to its end.
    overflow: bool,
    cap: usize,
    /// A UTF-8 character still arriving: its bytes so far, how many more it needs, and where it starts.
    pending: [u8; 4],
    pending_len: usize,
    pending_need: usize,
    pending_at: Place,
    /// Where the reader is, in lines and columns.
    cols: Columns,
}

impl WordReader {
    fn new(cap: usize) -> Self {
        Self {
            word: Vec::new(),
            start: Place::default(),
            dots: 0,
            overflow: false,
            cap,
            pending: [0; 4],
            pending_len: 0,
            pending_need: 0,
            pending_at: Place::default(),
            cols: Columns::new(),
        }
    }

    /// Reads `chunk`, which starts `chunk_start` bytes into the text, and hands each finished word to `on_word`.
    fn feed<F: FnMut(&Word<'_>)>(&mut self, chunk: &[u8], chunk_start: u64, on_word: &mut F) {
        for (i, &byte) in chunk.iter().enumerate() {
            let class = BYTE_CLASS[usize::from(byte)];
            if self.pending_need > 0 {
                if class == CONTINUES {
                    // A continuation byte never moves the column, but the counter must see every byte to spot a
                    // byte-order mark.
                    self.cols.advance(byte);
                    self.pending[self.pending_len] = byte;
                    self.pending_len += 1;
                    self.pending_need -= 1;
                    if self.pending_need == 0 {
                        let (bytes, len, at) = (self.pending, self.pending_len, self.pending_at);
                        self.pending_len = 0;
                        match std::str::from_utf8(&bytes[..len])
                            .ok()
                            .and_then(|text| text.chars().next())
                        {
                            Some(c) if is_word_char(c) => self.push(&bytes[..len], at),
                            // Not a word character, or not a character at all: an overlong form or a surrogate.
                            _ => self.split(on_word),
                        }
                    }
                    continue;
                }
                // The character was cut short. What arrived of it splits words, and this byte starts afresh.
                self.pending_need = 0;
                self.pending_len = 0;
                self.split(on_word);
            }
            let column = self.cols.column + 1;
            self.cols.advance(byte);
            match class {
                WORD_BYTE => {
                    let at = Place {
                        offset: chunk_start + i as u64,
                        line: self.cols.line,
                        column,
                    };
                    self.push(&[byte], at);
                }
                DOT => {
                    // Held back: a full stop belongs to the word only if more of the word follows. A leading one
                    // never does.
                    if !self.word.is_empty() {
                        self.dots += 1;
                    }
                }
                LEAD_2 | LEAD_3 | LEAD_4 => {
                    self.pending[0] = byte;
                    self.pending_len = 1;
                    self.pending_need = usize::from(class - LEAD_2) + 1;
                    self.pending_at = Place {
                        offset: chunk_start + i as u64,
                        line: self.cols.line,
                        column,
                    };
                }
                // A splitting byte, or a continuation byte with nothing to continue.
                _ => self.split(on_word),
            }
        }
    }

    /// Hands over the last word, at the end of the text.
    fn finish<F: FnMut(&Word<'_>)>(&mut self, on_word: &mut F) {
        self.pending_need = 0;
        self.pending_len = 0;
        self.split(on_word);
    }

    /// Adds one whole word character, starting at `at`.
    fn push(&mut self, bytes: &[u8], at: Place) {
        if self.overflow {
            // Still inside a run too long to be a word.
            return;
        }
        if self.word.len() + self.dots + bytes.len() > self.cap {
            self.overflow = true;
            self.word.clear();
            self.dots = 0;
            return;
        }
        if self.word.is_empty() {
            self.start = at;
        }
        if self.dots > 0 {
            // The held-back full stops turned out to be inside the word.
            self.word.resize(self.word.len() + self.dots, b'.');
            self.dots = 0;
        }
        self.word.extend_from_slice(bytes);
    }

    fn split<F: FnMut(&Word<'_>)>(&mut self, on_word: &mut F) {
        if !self.word.is_empty() && !self.overflow {
            // Only whole characters were pushed, so this always succeeds.
            if let Ok(text) = std::str::from_utf8(&self.word) {
                on_word(&Word {
                    text,
                    at: self.start,
                });
            }
        }
        self.word.clear();
        self.dots = 0;
        self.overflow = false;
    }
}

/// The closest a word came to one term: the term, the typos, and which bytes of the word matched.
#[derive(Debug, Clone, Copy)]
struct NearBest {
    term: usize,
    edits: usize,
    start: usize,
    end: usize,
}

/// Checks the words of one file against the terms that get near-misses, adding to the sweeper's tallies.
struct NearPass<'a> {
    terms: &'a NearTerms,
    searcher: NearSearcher<'a>,
    /// The watch list, for naming terms in hits.
    names: &'a [String],
    exact: bool,
    max_hits: usize,
    input: &'a str,
    hits: &'a mut Vec<SweepNearHit>,
    counts: &'a mut [u64],
    total: &'a mut u64,
    file_hits: u64,
    // Scratch space, kept from word to word.
    matches: Vec<NameMatch>,
    best: Vec<NearBest>,
    pieces: Vec<(usize, usize)>,
    folded: String,
}

impl NearPass<'_> {
    fn word(&mut self, word: &Word<'_>) {
        let text = word.text;
        let has_at = text.contains('@');
        // Most words in a dump can't be near any term on length alone: drop those before any copying. A word with an
        // @ goes on, because a piece of it may still be near something.
        if !has_at && !self.terms.set.could_match(text.chars().count()) {
            return;
        }
        // Capitals are folded unless matching is exact. Folding ASCII leaves every byte where it was, so byte
        // ranges in the folded word are byte ranges in the word as written.
        let compared: &str = if self.exact {
            text
        } else {
            self.folded.clear();
            self.folded.push_str(text);
            self.folded.make_ascii_lowercase();
            &self.folded
        };
        // The whole word first; then, when it holds an @, each piece either side of one.
        self.pieces.clear();
        self.pieces.push((0, text.len()));
        if has_at {
            let mut from = 0;
            for (at, _) in text.match_indices('@') {
                self.pieces.push((from, at));
                from = at + 1;
            }
            self.pieces.push((from, text.len()));
        }
        self.best.clear();
        for &(start, end) in &self.pieces {
            if start == end {
                continue;
            }
            self.matches.clear();
            self.searcher.find(&compared[start..end], &mut self.matches);
            for found in &self.matches {
                let (term, term_text) = &self.terms.terms[found.index];
                // A word holding the term as written is the exact sweep's hit, not a near-miss.
                if compared.contains(term_text.as_str()) {
                    continue;
                }
                let best = NearBest {
                    term: *term,
                    edits: found.edits,
                    start,
                    end,
                };
                // One near-miss per term per word: the closest piece, and on a tie the first, so the whole word wins.
                match self.best.iter().position(|b| b.term == *term) {
                    Some(i) if found.edits < self.best[i].edits => self.best[i] = best,
                    Some(_) => {}
                    None => self.best.push(best),
                }
            }
        }
        self.best
            .sort_unstable_by_key(|b| (b.start, b.edits, b.term));
        for best in &self.best {
            *self.total += 1;
            self.file_hits += 1;
            self.counts[best.term] += 1;
            if self.hits.len() < self.max_hits {
                self.hits.push(SweepNearHit {
                    term: self.names[best.term].clone(),
                    input: self.input.to_string(),
                    offset: word.at.offset + best.start as u64,
                    line: word.at.line,
                    column: word.at.column + text[..best.start].chars().count() as u64,
                    matched: text[best.start..best.end].to_string(),
                    edits: best.edits,
                });
            }
        }
    }
}

/// Runs a whole request: every file in order, then the report.
pub fn sweep_files(request: &SweepRequest) -> Result<SweepReport, SweepError> {
    if request.inputs.is_empty() {
        return Err(SweepError::NoInputs);
    }
    let mut sweeper = Sweeper::new(&request.terms, request.exact, request.max_hits)?
        .with_chunk_bytes(request.chunk_bytes)?
        .with_fuzzy(request.fuzzy)?;
    for path in &request.inputs {
        sweeper.sweep_file(path)?;
    }
    Ok(sweeper.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_lists_in_every_encoding_windows_writes() {
        let list = "# Lantern case\nlantern@example.org\r\n\n  +44 7700 900123  \n#lantern\n#\n";
        let want = vec!["lantern@example.org", "+44 7700 900123", "#lantern"];
        assert_eq!(parse_terms(list.as_bytes()).unwrap(), want);
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(list.as_bytes());
        assert_eq!(parse_terms(&bom).unwrap(), want);
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for unit in list.encode_utf16() {
            le.extend_from_slice(&unit.to_le_bytes());
            be.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(parse_terms(&le).unwrap(), want);
        assert_eq!(parse_terms(&be).unwrap(), want);
        assert!(matches!(
            parse_terms(&[0xC3, 0x28]),
            Err(SweepError::BadWatchList(_))
        ));
        assert!(matches!(
            parse_terms(&[0xFF, 0xFE, 0x41]),
            Err(SweepError::BadWatchList(_))
        ));
    }

    #[test]
    fn terms_are_checked_and_deduplicated() {
        let terms = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            Sweeper::new(&terms(&["", ""]), false, 10).unwrap_err(),
            SweepError::NoTerms
        );
        assert_eq!(
            Sweeper::new(&terms(&["two\nlines"]), false, 10).unwrap_err(),
            SweepError::TermSpansLines("two\nlines".into())
        );
        let folded = Sweeper::new(&terms(&["Moth", "moth", "MOTH"]), false, 10).unwrap();
        assert_eq!(folded.terms, vec!["Moth"]);
        let exact = Sweeper::new(&terms(&["Moth", "moth", "Moth"]), true, 10).unwrap();
        assert_eq!(exact.terms, vec!["Moth", "moth"]);
        assert!(matches!(
            Sweeper::new(&terms(&["x"]), false, 10)
                .unwrap()
                .with_chunk_bytes(0),
            Err(SweepError::BadSetting(_))
        ));
    }

    #[test]
    fn typos_allowed_follow_term_length() {
        for (chars, want) in [(0, 0), (1, 0), (4, 0), (5, 1), (7, 1), (8, 2), (40, 2)] {
            assert_eq!(edits_allowed(chars, 2), want, "{chars} characters");
            assert_eq!(edits_allowed(chars, 1), want.min(1), "{chars} characters");
            assert_eq!(edits_allowed(chars, 0), 0, "{chars} characters");
        }
    }

    #[test]
    fn words_are_letters_digits_and_six_marks() {
        for c in [
            'a', 'Z', '7', 'é', 'Ж', '中', '٣', '@', '.', '_', '-', '+', '#',
        ] {
            assert!(is_word_char(c), "{c:?}");
        }
        for c in [
            ' ', '\t', '\n', '\u{a0}', ',', ':', '/', '\'', '"', '“', '”', '—', '…', '(', '!', '😀',
        ] {
            assert!(!is_word_char(c), "{c:?}");
        }
        for term in ["@moth_relay", "lantern@example.org", "#lantern", "jdoe88"] {
            assert!(is_one_word(term), "{term:?}");
        }
        for term in [
            "+44 7700 900123",
            "O'Brien",
            ".onion",
            "example.",
            "",
            "a/b",
        ] {
            assert!(!is_one_word(term), "{term:?}");
        }
    }

    /// Every word the reader hands over, with where it starts, reading `bytes` in chunks of `chunk` bytes.
    fn words_of(bytes: &[u8], chunk: usize, cap: usize) -> Vec<(String, Place)> {
        let mut reader = WordReader::new(cap);
        let mut words = Vec::new();
        let mut keep = |word: &Word<'_>| words.push((word.text.to_string(), word.at));
        for (n, piece) in bytes.chunks(chunk).enumerate() {
            reader.feed(piece, (n * chunk) as u64, &mut keep);
        }
        reader.finish(&mut keep);
        words
    }

    fn place(offset: u64, line: u64, column: u64) -> Place {
        Place {
            offset,
            line,
            column,
        }
    }

    #[test]
    fn words_are_split_trimmed_and_kept_whole_across_chunks() {
        let text = "Ask jdeo88. Or ...@moth_rely, “Lantern@Example.org”\nnaïve—café x\u{a0}y";
        let want = vec![
            ("Ask".to_string(), place(0, 1, 1)),
            ("jdeo88".to_string(), place(4, 1, 5)),
            ("Or".to_string(), place(12, 1, 13)),
            ("@moth_rely".to_string(), place(18, 1, 19)),
            ("Lantern@Example.org".to_string(), place(33, 1, 32)),
            ("naïve".to_string(), place(56, 2, 1)),
            ("café".to_string(), place(65, 2, 7)),
            ("x".to_string(), place(71, 2, 12)),
            ("y".to_string(), place(74, 2, 14)),
        ];
        for chunk in 1..=9 {
            assert_eq!(
                words_of(text.as_bytes(), chunk, 4096),
                want,
                "chunk {chunk}"
            );
        }
        assert_eq!(words_of(text.as_bytes(), 1 << 20, 4096), want);
    }

    #[test]
    fn bytes_that_are_not_utf8_split_words() {
        // A stray 0xFF, a euro sign cut short by "x", an overlong slash, and a character cut short by the end.
        // Columns follow the rule for hits: every byte but a continuation byte counts, valid or not.
        let bytes = b"ab\xFFcd \xE2\x82x \xC0\xAFok \xC3";
        let want = vec![
            ("ab".to_string(), place(0, 1, 1)),
            ("cd".to_string(), place(3, 1, 4)),
            ("x".to_string(), place(8, 1, 8)),
            ("ok".to_string(), place(12, 1, 11)),
        ];
        for chunk in 1..=5 {
            assert_eq!(words_of(bytes, chunk, 4096), want, "chunk {chunk}");
        }
    }

    #[test]
    fn a_leading_byte_order_mark_is_not_a_column() {
        // (line, column) after each character of the text, read a byte at a time.
        let after_each = |text: &str| -> Vec<(u64, u64)> {
            let mut cols = Columns::new();
            let mut out = Vec::new();
            for (i, byte) in text.bytes().enumerate() {
                cols.advance(byte);
                let ends_a_char = text.is_char_boundary(i + 1);
                if ends_a_char {
                    out.push((cols.line, cols.column));
                }
            }
            out
        };
        // The mark leaves the count at zero, so "a" is column 1, as an editor shows it.
        assert_eq!(
            after_each("\u{feff}ab\ncd"),
            vec![(1, 0), (1, 1), (1, 2), (2, 0), (2, 1), (2, 2)]
        );
        // Anywhere else, U+FEFF is an ordinary character.
        assert_eq!(after_each("a\u{feff}b"), vec![(1, 1), (1, 2), (1, 3)]);
        // A character that only starts like the mark (EF B8 80) still counts.
        assert_eq!(after_each("\u{fe00}a"), vec![(1, 1), (1, 2)]);
        // A mark cut short counts as what it is: one stray character, then the line feed.
        let mut cols = Columns::new();
        for &byte in b"\xEF\xBB\nx" {
            cols.advance(byte);
        }
        assert_eq!((cols.line, cols.column), (2, 1));
        // Words after a mark start at column 1 at every chunk size.
        let text = "\u{feff}jdeo88 moth";
        for chunk in 1..=5 {
            assert_eq!(
                words_of(text.as_bytes(), chunk, 4096),
                vec![
                    ("jdeo88".to_string(), place(3, 1, 1)),
                    ("moth".to_string(), place(10, 1, 8)),
                ],
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn runs_longer_than_the_cap_are_skipped() {
        let text = "abcdefghij jdeo88 ab.......cd jdeo88........ end";
        let words: Vec<String> = words_of(text.as_bytes(), 3, 8)
            .into_iter()
            .map(|(word, _)| word)
            .collect();
        assert_eq!(words, vec!["jdeo88", "jdeo88", "end"]);
    }

    #[test]
    fn fuzzy_settings_are_checked() {
        let terms = vec!["lantern@example.org".to_string()];
        let sweeper = Sweeper::new(&terms, false, 10).unwrap();
        assert!(matches!(
            sweeper.clone().with_fuzzy(3),
            Err(SweepError::BadSetting(_))
        ));
        let off = sweeper.clone().with_fuzzy(0).unwrap();
        assert!(off.near.is_none());
        let on = sweeper.with_fuzzy(1).unwrap();
        assert_eq!(on.allowed, vec![1]);
        assert!(on.near.is_some());
    }
}
