use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use text_splitter::{ChunkConfig, TextSplitter};
use tiktoken_rs::{get_bpe_from_model, CoreBPE};

lazy_static! {
    static ref RE_WHITESPACES: Regex = Regex::new(r"\s+").unwrap();
    static ref RE_DASHES: Regex = Regex::new(r"-{2,}").unwrap();
    static ref RE_UNDERSCORES: Regex = Regex::new(r"_{2,}").unwrap();
    static ref RE_XO: Regex = Regex::new(r"\x00").unwrap();
}

/// Configuration for text splitting
#[derive(Debug, Clone)]
pub struct SplitConfig {
    pub max_tokens: usize,
    pub words_to_tokens_ratio: f64, // Conservative estimate: 1 word ≈ 1.5 tokens
    pub check_threshold: f64,       // Check with tiktoken when at 80% of limit
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            max_tokens: 300,
            words_to_tokens_ratio: 1.5, // Conservative: assume 1 word = 1.5 tokens
            check_threshold: 0.7,       // Check when at 70% of limit for stricter enforcement
        }
    }
}

/// Preprocess text EXACTLY like the implementation you provided
pub fn preprocess_text(text: &str) -> String {
    // First handle hyphenated words at line breaks
    let text = fix_hyphenated_words(text);

    // Handle repeated characters without backreferences
    let mut cleaned_text = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        cleaned_text.push(ch);
        let mut count = 1;

        // Count consecutive identical characters
        while chars.peek() == Some(&ch) {
            chars.next();
            count += 1;
        }

        // If we had 4 or more of the same character, replace with exactly 3
        if count >= 4 {
            // We already pushed one, so push 2 more
            cleaned_text.push(ch);
            cleaned_text.push(ch);
        } else if count > 1 {
            // Push the remaining characters (we already pushed one)
            for _ in 1..count {
                cleaned_text.push(ch);
            }
        }
    }

    // Apply the other regex replacements
    let cleaned_text = RE_WHITESPACES.replace_all(&cleaned_text, " ");
    let cleaned_text = RE_DASHES.replace_all(&cleaned_text, "-");
    let cleaned_text = RE_UNDERSCORES.replace_all(&cleaned_text, "_");
    let cleaned_text = RE_XO.replace_all(&cleaned_text, r"\ 0");

    cleaned_text.trim().to_string()
}

/// Fix hyphenated words that are split across lines
fn fix_hyphenated_words(text: &str) -> String {
    lazy_static! {
        // Match word-hyphen at end of line followed by continuation on next line
        static ref HYPHEN_NEWLINE: Regex = Regex::new(r"(\w+)-\s*\n\s*(\w+)").unwrap();
    }

    // Replace hyphen-newline-word patterns with just the combined word
    HYPHEN_NEWLINE.replace_all(text, "$1$2").to_string()
}

/// Get a text splitter configured like the implementation you provided
pub fn get_text_splitter(
    max_tokens: usize,
) -> Result<TextSplitter<tiktoken_rs::CoreBPE>, Box<dyn std::error::Error>> {
    let tokenizer = get_bpe_from_model("gpt-4o")?;
    Ok(TextSplitter::new(
        ChunkConfig::new(max_tokens).with_sizer(tokenizer),
    ))
}

lazy_static! {
    /// Create a shared tokenizer instance for efficiency
    static ref SHARED_TOKENIZER: Option<CoreBPE> = {
        match get_bpe_from_model("gpt-4o") {
            Ok(tokenizer) => Some(tokenizer),
            Err(_) => None,
        }
    };
}

/// Get a text splitter using the shared tokenizer
pub fn get_text_splitter_shared(
    max_tokens: usize,
) -> Result<TextSplitter<tiktoken_rs::CoreBPE>, Box<dyn std::error::Error>> {
    match &*SHARED_TOKENIZER {
        Some(tokenizer) => Ok(TextSplitter::new(
            ChunkConfig::new(max_tokens).with_sizer(tokenizer.clone()),
        )),
        None => get_text_splitter(max_tokens), // Fallback to creating new tokenizer
    }
}

/// Estimate token count from word count using heuristic
pub fn estimate_tokens_from_words(word_count: usize, ratio: f64) -> usize {
    (word_count as f64 * ratio).ceil() as usize
}

/// Count words in text
pub fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Check if we should verify token count with tiktoken
pub fn should_check_tokens(estimated_tokens: usize, max_tokens: usize, threshold: f64) -> bool {
    estimated_tokens >= (max_tokens as f64 * threshold) as usize
}

/// Get actual token count using tiktoken
pub fn get_token_count(text: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let tokenizer = get_bpe_from_model("gpt-4o")?;
    let allowed_special = HashSet::new();
    let (tokens, _) = tokenizer.encode(text, &allowed_special);
    Ok(tokens.len())
}

/// Efficient token counting with caching for repeated checks
pub struct TokenCounter {
    bpe: tiktoken_rs::CoreBPE,
    cache: std::collections::HashMap<u64, usize>,
}

impl TokenCounter {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let bpe = get_bpe_from_model("gpt-4o")?; // Using gpt-4o tokenizer
        Ok(TokenCounter {
            bpe,
            cache: std::collections::HashMap::new(),
        })
    }

    pub fn count_tokens(&mut self, text: &str) -> usize {
        // Use a fast hash for caching
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();

        // Check cache first
        if let Some(&count) = self.cache.get(&hash) {
            return count;
        }

        // Count tokens
        let allowed_special = HashSet::new();
        let (tokens, _) = self.bpe.encode(text, &allowed_special);
        let count = tokens.len();

        // Cache the result (limit cache size to prevent memory issues)
        if self.cache.len() < 10000 {
            self.cache.insert(hash, count);
        }

        count
    }

    pub fn estimate_tokens(&self, text: &str) -> usize {
        // Fast estimation without actual tokenization
        let word_count = count_words(text);
        estimate_tokens_from_words(word_count, 1.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_text() {
        assert_eq!(preprocess_text("hello    world"), "hello world");
        assert_eq!(preprocess_text("test----text"), "test-text");
        assert_eq!(preprocess_text("aaaa"), "aaa");
        assert_eq!(preprocess_text("aaaaa"), "aaa");
        assert_eq!(preprocess_text("aaa"), "aaa");
        assert_eq!(preprocess_text("aa"), "aa");
    }

    #[test]
    fn test_word_count() {
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("  multiple   spaces  "), 2);
        assert_eq!(count_words(""), 0);
    }
}
