// Non-code file parsers for structural analysis.
// These use regex/simple parsing rather than tree-sitter.
// They produce SectionInfo (document sections) rather than functions/classes.

use serde::{Deserialize, Serialize};

/// A section in a non-code file (e.g., Markdown heading, YAML key, SQL table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub name: String,
    pub level: u32,
    #[serde(rename = "lineRange")]
    pub line_range: [u32; 2],
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// Parse Markdown sections (ATX headings).
/// Skips content inside code fences.
pub fn parse_markdown_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();
    let mut in_code_fence = false;
    let mut code_fence_start = 0;

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Track code fences
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if !in_code_fence {
                in_code_fence = true;
                code_fence_start = line_idx;
            } else {
                in_code_fence = false;
            }
            continue;
        }

        if in_code_fence {
            continue;
        }

        // ATX heading: 1-6 # characters
        let heading_level = trimmed
            .chars()
            .take_while(|&c| c == '#')
            .count();

        if heading_level > 0 && heading_level <= 6 {
            let name = trimmed[heading_level..].trim();
            if !name.is_empty() {
                sections.push(SectionInfo {
                    name: name.to_string(),
                    level: heading_level as u32,
                    line_range: [line_idx as u32 + 1, line_idx as u32 + 1],
                });
            }
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------------

/// Parse YAML sections (top-level keys).
/// Matches lines like `key:` at the start of a line (with possible whitespace).
pub fn parse_yaml_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();
    let line_count = content.lines().count();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip blank lines, comments, and indented lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Top-level bare key: at column 0 (modulo leading whitespace), ends with colon but not key: value
        if trimmed.ends_with(':') && !trimmed.contains(": ") {
            let key = trimmed.trim_end_matches(':').trim();

            // Verify this looks like a YAML key (not a flow mapping or nested)
            if !key.is_empty() && !key.contains("{") && !key.contains("}") && !key.contains("[") && !key.contains("]") {
                // Check it's actually at the top level (leading spaces < 2)
                let leading_spaces = line.len() - line.trim_start().len();
                if leading_spaces < 2 {
                    sections.push(SectionInfo {
                        name: key.to_string(),
                        level: 1,
                        line_range: [line_idx as u32 + 1, line_idx as u32 + 1],
                    });
                }
            }
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Parse JSON sections (top-level object keys).
pub fn parse_json_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();
    let trimmed = content.trim();

    // Find the outermost object
    let start = match trimmed.find('{') {
        Some(pos) => pos,
        None => return sections,
    };

    // Track nesting level and whether we're in a string
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut key_start = None;

    for (i, c) in trimmed[start..].chars().enumerate() {
        let absolute_pos = start + i;

        if escape_next {
            escape_next = false;
            continue;
        }

        if c == '\\' {
            escape_next = true;
            continue;
        }

        if c == '"' {
            if !in_string {
                in_string = true;
                key_start = Some(absolute_pos + 1);
            } else {
                in_string = false;
                // We have a key - find the colon after it
                let key_end = absolute_pos;
                let key = &trimmed[key_start.unwrap_or(0)..key_end];

                // Look for colon after the closing quote
                for j in (key_end + 1)..trimmed.len() {
                    match trimmed.chars().nth(j) {
                        Some(' ') | Some('\n') | Some('\t') | Some('\r') => continue,
                        Some(':') => {
                            // This is a key-value pair
                            if depth == 1 && !key.is_empty() {
                                sections.push(SectionInfo {
                                    name: key.to_string(),
                                    level: 1,
                                    line_range: [1, 1], // JSON is typically single-line, approximate
                                });
                            }
                            break;
                        }
                        _ => break,
                    }
                }
            }
            continue;
        }

        if in_string {
            continue;
        }

        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// TOML
// ---------------------------------------------------------------------------

/// Parse TOML sections (top-level section headers).
/// Matches `[section.name]` style headers.
pub fn parse_toml_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip blank lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Section header: [section.name]
        if let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = inner.trim();
            // Skip table array syntax [[array]]
            if !name.is_empty() && !name.starts_with('[') {
                sections.push(SectionInfo {
                    name: name.to_string(),
                    level: 1,
                    line_range: [line_idx as u32 + 1, line_idx as u32 + 1],
                });
            }
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

/// Parse SQL sections (CREATE TABLE and CREATE INDEX statements).
pub fn parse_sql_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();
    let keywords = ["CREATE TABLE", "CREATE INDEX", "CREATE VIEW", "CREATE FUNCTION",
                    "CREATE PROCEDURE", "CREATE TRIGGER"];

    for (line_idx, line) in content.lines().enumerate() {
        let upper = line.to_uppercase();
        let trimmed = upper.trim();

        for keyword in &keywords {
            let kw_upper = keyword.to_uppercase();
            if trimmed.starts_with(&kw_upper) {
                // Extract the name after the keyword
                let rest = &line[kw_upper.len()..].trim();
                // Name is typically the next word or quoted identifier
                let name = rest.split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
                    .to_string();

                if !name.is_empty() {
                    sections.push(SectionInfo {
                        name,
                        level: 1,
                        line_range: [line_idx as u32 + 1, line_idx as u32 + 1],
                    });
                }
                break;
            }
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// Dockerfile
// ---------------------------------------------------------------------------

/// Parse Dockerfile sections (FROM, RUN, CMD, LABEL, EXPOSE, etc.).
pub fn parse_dockerfile_sections(content: &str) -> Vec<SectionInfo> {
    let mut sections = Vec::new();
    let instructions = ["FROM", "RUN", "CMD", "LABEL", "EXPOSE", "ENV", "ADD", "COPY",
                       "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG", "ONBUILD",
                       "STOPSIGNAL", "HEALTHCHECK", "SHELL", "MAINTAINER", "CROSSBUILD"];

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip blank lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Parse instruction
        let upper = trimmed.to_uppercase();
        for instr in &instructions {
            let instr_upper = instr.to_uppercase();
            if upper.starts_with(&instr_upper) {
                let rest = trimmed[instr.len()..].trim();
                let name = if rest.is_empty() {
                    instr.to_string()
                } else {
                    // Get the first argument (typically the image name or command)
                    let first_arg = rest.split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if first_arg.is_empty() {
                        instr.to_string()
                    } else {
                        format!("{} {}", instr, first_arg)
                    }
                };

                sections.push(SectionInfo {
                    name,
                    level: 1,
                    line_range: [line_idx as u32 + 1, line_idx as u32 + 1],
                });
                break;
            }
        }
    }

    sections
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_sections() {
        let md = "# Title\n\n## Section 1\n\n### SubSection\n\ntext\n\n```\n# not a heading\n```\n\n## Section 2";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].name, "Title");
        assert_eq!(sections[0].level, 1);
        assert_eq!(sections[1].name, "Section 1");
        assert_eq!(sections[1].level, 2);
        assert_eq!(sections[2].name, "SubSection");
        assert_eq!(sections[2].level, 3);
        assert_eq!(sections[3].name, "Section 2");
    }

    #[test]
    fn test_yaml_sections() {
        // YAML keys with values (key: value) and bare keys (key:)
        let yaml = "name: test\nversion: 1.0\n\nnested:\n  key: value\n\nother: top";
        let sections = parse_yaml_sections(yaml);
        // Bare keys like "nested:" are picked up; keys with values are not
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, "nested");
    }

    #[test]
    fn test_toml_sections() {
        let toml = "[package]\nname = \"test\"\n\n[dependencies]\n\n[profile.release]";
        let sections = parse_toml_sections(toml);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].name, "package");
        assert_eq!(sections[1].name, "dependencies");
        assert_eq!(sections[2].name, "profile.release");
    }

    #[test]
    fn test_sql_sections() {
        let sql = "CREATE TABLE users (id INT);\nCREATE INDEX idx ON users(id);\nCREATE VIEW v AS SELECT * FROM users;";
        let sections = parse_sql_sections(sql);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].name, "users");
        assert_eq!(sections[1].name, "idx");
        assert_eq!(sections[2].name, "v");
    }

    #[test]
    fn test_dockerfile_sections() {
        let df = "FROM ubuntu:20.04\nRUN apt-get update\nCMD /bin/bash\nEXPOSE 8080";
        let sections = parse_dockerfile_sections(df);
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].name, "FROM ubuntu:20.04");
        assert_eq!(sections[1].name, "RUN apt-get");
        assert_eq!(sections[2].name, "CMD /bin/bash");
        assert_eq!(sections[3].name, "EXPOSE 8080");
    }
}
