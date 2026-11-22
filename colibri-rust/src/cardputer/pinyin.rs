use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

const MAX_SPELLING: usize = 64;
const BEAM: usize = 5;
const MAX_CANDIDATES: usize = 48;

#[derive(Clone, Serialize)]
pub(super) struct Candidate {
    text: String,
    consumed: usize,
}

struct Word {
    text: String,
    score: f64,
    boundaries: u64,
}

type Lexicon = HashMap<String, Vec<Word>>;

fn singles() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("../../assets/cardputer-pinyin.tsv")
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    })
}

pub(super) fn single_characters(spelling: &str) -> &'static str {
    singles().get(spelling).copied().unwrap_or("")
}

fn lexicon() -> &'static Lexicon {
    static WORDS: OnceLock<Lexicon> = OnceLock::new();
    WORDS.get_or_init(|| {
        let installed = Path::new("/usr/share/colibri-cardputer/pinyin/words.tsv");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../cardputer-zero/assets/pinyin/words.tsv");
        let contents = std::fs::read_to_string(if source.is_file() { &source } else { installed })
            .unwrap_or_default();
        let mut words: Lexicon = HashMap::new();
        for line in contents.lines() {
            let mut fields = line.split('\t');
            let (Some(spelling), Some(text), Some(frequency)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(frequency) = frequency.parse::<u32>() else {
                continue;
            };
            let mut key = String::new();
            let mut boundaries = 0;
            for syllable in spelling.split_whitespace() {
                if !key.is_empty() && key.len() < MAX_SPELLING {
                    boundaries |= 1 << key.len();
                }
                key.push_str(syllable);
            }
            let entries = words.entry(key).or_default();
            // The lexicon is preordered by descending frequency for each spelling.
            if entries.len() < MAX_CANDIDATES {
                entries.push(Word {
                    text: text.to_owned(),
                    score: f64::from(frequency.max(1)).log10() - 9.5,
                    boundaries,
                });
            }
        }
        words
    })
}

fn add_candidate(result: &mut Vec<Candidate>, text: String, consumed: usize) {
    if !result
        .iter()
        .any(|candidate| candidate.text == text && candidate.consumed == consumed)
    {
        result.push(Candidate { text, consumed });
    }
}

pub(super) fn candidates(spelling: &str) -> Vec<Candidate> {
    if spelling.is_empty()
        || spelling.len() > MAX_SPELLING
        || spelling.starts_with('\'')
        || spelling.contains("''")
        || !spelling
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\'')
    {
        return Vec::new();
    }
    let words = lexicon();
    let length = spelling.len();
    let mut best: Vec<Vec<(f64, String)>> = vec![Vec::new(); length + 1];
    best[length].push((0.0, String::new()));
    for start in (0..length).rev() {
        if spelling.as_bytes()[start] == b'\'' {
            best[start] = best[start + 1].clone();
            continue;
        }
        let mut paths = Vec::new();
        let mut key = String::new();
        let mut boundaries = 0;
        for end in start + 1..=length {
            let letter = spelling.as_bytes()[end - 1];
            if letter == b'\'' {
                boundaries |= 1 << key.len();
                continue;
            }
            key.push(char::from(letter));
            if best[end].is_empty() {
                continue;
            }
            if let Some(entries) = words.get(&key) {
                for word in entries
                    .iter()
                    .filter(|word| word.boundaries & boundaries == boundaries)
                    .take(BEAM)
                {
                    for (score, tail) in &best[end] {
                        paths.push((word.score + score, format!("{}{tail}", word.text)));
                    }
                }
            } else if let Some(character) = single_characters(&spelling[start..end]).chars().next()
            {
                for (score, tail) in &best[end] {
                    paths.push((score - 12.0, format!("{character}{tail}")));
                }
            }
        }
        paths.sort_by(|left, right| right.0.total_cmp(&left.0));
        let mut seen = std::collections::HashSet::new();
        paths.retain(|(_, text)| seen.insert(text.clone()));
        paths.truncate(BEAM);
        best[start] = paths;
    }

    let mut result = Vec::new();
    if let Some(entries) = words.get(spelling) {
        for word in entries.iter().take(BEAM) {
            add_candidate(&mut result, word.text.clone(), length);
        }
    }
    for (_, text) in best[0].drain(..) {
        add_candidate(&mut result, text, length);
    }

    let first_boundary = spelling.find('\'').unwrap_or(length);
    for end in (1..=first_boundary).rev() {
        let key = &spelling[..end];
        let consumed = end + usize::from(spelling.as_bytes().get(end) == Some(&b'\''));
        let is_full = consumed == length;
        if let Some(entries) = words.get(key) {
            for word in entries
                .iter()
                .take(if is_full { MAX_CANDIDATES } else { 3 })
            {
                add_candidate(&mut result, word.text.clone(), consumed);
            }
        }
        for character in single_characters(key)
            .chars()
            .take(if is_full { usize::MAX } else { 3 })
        {
            add_candidate(&mut result, character.to_string(), consumed);
        }
    }
    if single_characters(spelling.trim_end_matches('\'')).is_empty() {
        result.truncate(MAX_CANDIDATES);
    }
    result
}
