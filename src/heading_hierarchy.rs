//! Header hierarchy management for PDF extraction
//!
//! This module provides a consistent way to manage header hierarchies,
//! ensuring we maintain a clean h1-h6 structure.

use crate::TextLevel;

/// Manages header hierarchy for a document
///
/// Rules:
/// 1. When encountering a lower level (e.g., H2 after H1), push to stack
/// 2. When encountering same level, replace the last one
/// 3. When encountering higher level, pop until that level and append
#[derive(Debug, Clone)]
pub struct HeaderHierarchy {
    stack: Vec<(u8, String)>, // (level, text) where level is 1-6
}

impl HeaderHierarchy {
    /// Create a new empty hierarchy
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Add a header to the hierarchy
    pub fn push(&mut self, level: TextLevel, text: String) {
        let level_num = match level {
            TextLevel::H1 => 1,
            TextLevel::H2 => 2,
            TextLevel::H3 => 3,
            TextLevel::H4 => 4,
            TextLevel::H5 => 5,
            TextLevel::H6 => 6,
            _ => return, // Skip non-header levels (Body, SubBody)
        };

        // Find where this header should go in the hierarchy
        let mut pop_to_index = self.stack.len();

        for (i, (existing_level, _)) in self.stack.iter().enumerate().rev() {
            match level_num.cmp(existing_level) {
                std::cmp::Ordering::Less => {
                    // New header is higher level (e.g., H1 when we have H2)
                    // Pop everything at this level and below
                    pop_to_index = i;
                }
                std::cmp::Ordering::Equal => {
                    // Same level - replace this one
                    pop_to_index = i;
                    break;
                }
                std::cmp::Ordering::Greater => {
                    // New header is lower level (e.g., H3 when we have H2)
                    // Keep everything and append
                    break;
                }
            }
        }

        // Pop headers as needed
        self.stack.truncate(pop_to_index);

        // Add the new header
        self.stack.push((level_num, text));
    }

    /// Get the current header hierarchy as a vector of strings
    pub fn get_headers(&self) -> Vec<String> {
        self.stack.iter().map(|(_, text)| text.clone()).collect()
    }

    /// Get the current header at a specific level (1-6)
    pub fn get_header_at_level(&self, level: u8) -> Option<&str> {
        self.stack
            .iter()
            .find(|(l, _)| *l == level)
            .map(|(_, text)| text.as_str())
    }

    /// Clear the hierarchy
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Get the deepest level in the current hierarchy
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Get the current heading (the last one added)
    pub fn current_heading(&self) -> Option<&str> {
        self.stack.last().map(|(_, text)| text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_hierarchy() {
        let mut hierarchy = HeaderHierarchy::new();

        // Add H1
        hierarchy.push(TextLevel::H1, "Chapter 1".to_string());
        assert_eq!(hierarchy.get_headers(), vec!["Chapter 1"]);

        // Add H2 (should append)
        hierarchy.push(TextLevel::H2, "Section 1.1".to_string());
        assert_eq!(hierarchy.get_headers(), vec!["Chapter 1", "Section 1.1"]);

        // Add another H2 (should replace the last H2)
        hierarchy.push(TextLevel::H2, "Section 1.2".to_string());
        assert_eq!(hierarchy.get_headers(), vec!["Chapter 1", "Section 1.2"]);

        // Add H3 (should append)
        hierarchy.push(TextLevel::H3, "Subsection 1.2.1".to_string());
        assert_eq!(
            hierarchy.get_headers(),
            vec!["Chapter 1", "Section 1.2", "Subsection 1.2.1"]
        );

        // Add H1 (should clear everything and start fresh)
        hierarchy.push(TextLevel::H1, "Chapter 2".to_string());
        assert_eq!(hierarchy.get_headers(), vec!["Chapter 2"]);
    }
}
