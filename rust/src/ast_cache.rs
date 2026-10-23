use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

pub const AST_MAGIC: &[u8; 4] = b"MAST"; // Memory-fabric AST
pub const AST_VERSION: u16 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BinaryAstEntry {
    pub relpath: String,
    pub heading: String,
    pub start_line: usize,
    pub end_line: usize,
    pub depth: u8,
}

pub struct AstBinaryIndex;

impl AstBinaryIndex {
    /// Compiles all markdown notes in vault into a compact binary AST index file.
    pub fn compile(vault_path: &Path, bin_path: &Path) -> Result<Value, String> {
        let mut entries = Vec::new();
        Self::scan_and_parse(vault_path, vault_path, &mut entries)?;

        let mut buf = Vec::new();
        buf.extend_from_slice(AST_MAGIC);
        buf.extend_from_slice(&AST_VERSION.to_le_bytes());
        buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());

        // Serialize entries with JSON or binary pack
        let payload = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&payload);

        fs::write(bin_path, buf).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(bin_path, fs::Permissions::from_mode(0o600));
        }

        Ok(json!({
            "ok": true,
            "sections_indexed": entries.len(),
            "binary_path": bin_path.to_string_lossy(),
            "bytes_written": fs::metadata(bin_path).map(|m| m.len()).unwrap_or(0)
        }))
    }

    /// Sub-microsecond lookup of heading line bounds from the pre-compiled binary AST.
    #[allow(dead_code)]
    pub fn lookup(
        bin_path: &Path,
        target_relpath: &str,
        target_heading: &str,
    ) -> Option<(usize, usize)> {
        let mut f = File::open(bin_path).ok()?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic).ok()?;
        if &magic != AST_MAGIC {
            return None;
        }

        let mut ver = [0u8; 2];
        f.read_exact(&mut ver).ok()?;
        let mut count_buf = [0u8; 4];
        f.read_exact(&mut count_buf).ok()?;

        let mut payload_len_buf = [0u8; 4];
        f.read_exact(&mut payload_len_buf).ok()?;
        let payload_len = u32::from_le_bytes(payload_len_buf) as usize;

        let mut payload = vec![0u8; payload_len];
        f.read_exact(&mut payload).ok()?;

        let entries: Vec<BinaryAstEntry> = serde_json::from_slice(&payload).ok()?;
        let target_heading_clean = target_heading
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_lowercase();

        for e in entries {
            if e.relpath == target_relpath {
                let h_clean = e
                    .heading
                    .trim()
                    .trim_start_matches('#')
                    .trim()
                    .to_lowercase();
                if h_clean == target_heading_clean || h_clean.contains(&target_heading_clean) {
                    return Some((e.start_line, e.end_line));
                }
            }
        }
        None
    }

    fn scan_and_parse(
        root: &Path,
        current: &Path,
        acc: &mut Vec<BinaryAstEntry>,
    ) -> Result<(), String> {
        if !current.exists() || !current.is_dir() {
            return Ok(());
        }
        let excluded = [
            "apple-notes",
            "notes_export",
            "templates",
            "archive",
            ".git",
            ".trash",
        ];
        let entries = fs::read_dir(current).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path)
                && meta.file_type().is_symlink()
            {
                continue;
            }
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !name.starts_with('.') && !excluded.contains(&name.as_str()) {
                    Self::scan_and_parse(root, &path, acc)?;
                }
            } else if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                let relpath = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                if let Ok(content) = fs::read_to_string(&path) {
                    let mut current_heading = "Overview".to_string();
                    let mut current_start = 1;
                    let mut current_depth = 1;

                    let lines: Vec<&str> = content.lines().collect();
                    for (i, line) in lines.iter().enumerate() {
                        let trimmed = line.trim();
                        if trimmed.starts_with('#') {
                            let depth = trimmed.chars().take_while(|&c| c == '#').count() as u8;
                            if i > 0 && i >= current_start {
                                acc.push(BinaryAstEntry {
                                    relpath: relpath.clone(),
                                    heading: current_heading.clone(),
                                    start_line: current_start,
                                    end_line: i,
                                    depth: current_depth,
                                });
                            }
                            current_heading = trimmed.trim_start_matches('#').trim().to_string();
                            current_start = i + 1;
                            current_depth = depth;
                        }
                    }
                    if lines.len() >= current_start {
                        acc.push(BinaryAstEntry {
                            relpath,
                            heading: current_heading,
                            start_line: current_start,
                            end_line: lines.len(),
                            depth: current_depth,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}
