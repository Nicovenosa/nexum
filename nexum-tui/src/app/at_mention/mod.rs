pub mod file_search;
pub mod popup;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_search::Entry;

/// @ 提及状态：管理文件搜索候选、选择和弹窗
pub struct AtMentionState {
    pub active: bool,
    pub query: String,
    /// @ 符号在文本中的字符位置
    pub query_start: usize,
    pub candidates: Vec<Entry>,
    pub selected: usize,
    pub scroll_offset: usize,
    /// 当前解析出的目录部分（已 resolve 的相对路径，含尾 /）
    dir_part: String,
    /// 工作目录
    cwd: PathBuf,
    /// 目录条目缓存：PathBuf → Vec<Entry>
    dir_cache: HashMap<PathBuf, Vec<Entry>>,
    /// 空列表占位消息："(empty directory)" / "(no matches)" / None
    pub empty_message: Option<String>,
}

impl Default for AtMentionState {
    fn default() -> Self {
        Self::new()
    }
}

impl AtMentionState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            query_start: 0,
            candidates: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            dir_part: String::new(),
            cwd: PathBuf::new(),
            dir_cache: HashMap::new(),
            empty_message: None,
        }
    }

    /// 惰性设置 cwd（仅在变更时重新设置）
    pub fn set_cwd(&mut self, cwd: String) {
        let new_path = PathBuf::from(&cwd);
        if self.cwd != new_path {
            self.dir_cache.clear();
            self.cwd = new_path;
        }
    }

    /// 确保 cwd 已设置（惰性初始化，仅设置一次）
    pub fn ensure_cwd(&mut self, cwd: String) {
        if self.cwd.as_os_str().is_empty() {
            self.set_cwd(cwd);
        }
    }

    /// detect 保持不变：从文本和光标位置检测 @ 提及
    pub fn detect(text: &str, cursor_pos: usize) -> Option<(String, usize)> {
        if cursor_pos == 0 || cursor_pos > text.len() {
            return None;
        }
        let before_cursor = &text[..cursor_pos];
        let at_pos = before_cursor.rfind('@')?;
        let query = &before_cursor[at_pos + '@'.len_utf8()..];
        if query.is_empty() {
            return None;
        }
        if at_pos > 0 {
            let char_before = before_cursor[..at_pos].chars().next_back().unwrap();
            if !char_before.is_whitespace() && char_before != '\n' {
                return None;
            }
        }
        Some((query.to_string(), at_pos))
    }

    pub fn activate(&mut self, query: String, query_start: usize) {
        self.active = true;
        self.query = query;
        self.query_start = query_start;
        self.selected = 0;
        self.scroll_offset = 0;
        self.empty_message = None;
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.candidates.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.dir_part.clear();
        self.empty_message = None;
    }

    /// 同步刷新候选列表：解析 query → resolve_dir → read_dir (cached) → fuzzy_match
    pub fn refresh_candidates(&mut self) {
        self.empty_message = None;

        if self.query.is_empty() {
            return;
        }

        let (dir_part, query_part) = match file_search::parse_at_query(&self.query) {
            Some(v) => v,
            None => return,
        };

        // 解析目录，不存在时回退
        let (resolved_dir, fallback) = file_search::resolve_dir(&self.cwd, &dir_part);

        // 存储解析后的相对目录路径（resolved → relative to cwd）
        let rel_resolved = resolved_dir
            .strip_prefix(&self.cwd)
            .unwrap_or(&resolved_dir);
        let mut resolved_prefix = rel_resolved.to_string_lossy().to_string();
        if !resolved_prefix.is_empty() && !resolved_prefix.ends_with('/') {
            resolved_prefix.push('/');
        }
        self.dir_part = resolved_prefix;

        // 合并 fallback 和 query_part 作为 fuzzy query
        let effective_query = if fallback.is_empty() {
            query_part.as_str()
        } else if query_part.is_empty() {
            fallback.as_str()
        } else {
            return self.refresh_candidates_with_query(
                &resolved_dir,
                &format!("{}/{}", fallback, query_part),
            );
        };

        self.refresh_candidates_with_query(&resolved_dir, effective_query);
    }

    fn refresh_candidates_with_query(&mut self, dir: &Path, query: &str) {
        let entries = self.get_or_read_dir(dir);

        if entries.is_empty() {
            self.candidates.clear();
            self.empty_message = Some("(empty directory)".to_string());
            return;
        }

        let matched = file_search::fuzzy_match_entries(entries, query);

        if matched.is_empty() && !query.is_empty() {
            self.candidates.clear();
            self.empty_message = Some("(no matches)".to_string());
            return;
        }

        self.candidates = matched.into_iter().cloned().collect();
        if self.selected >= self.candidates.len() && !self.candidates.is_empty() {
            self.selected = self.candidates.len() - 1;
        }
    }

    fn get_or_read_dir(&mut self, dir: &Path) -> &Vec<Entry> {
        if !self.dir_cache.contains_key(dir) {
            let entries = file_search::read_dir_entries(dir);
            self.dir_cache.insert(dir.to_path_buf(), entries);
        }
        self.dir_cache.get(dir).unwrap()
    }

    pub fn move_up(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            self.selected = self.candidates.len() - 1;
        }
        self.adjust_scroll();
    }

    pub fn move_down(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        if self.selected < self.candidates.len() - 1 {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
        self.adjust_scroll();
    }

    pub fn adjust_scroll(&mut self) {
        let viewport = popup::MAX_VIEWPORT.min(self.candidates.len());
        if viewport == 0 {
            self.scroll_offset = 0;
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + viewport {
            self.scroll_offset = self.selected - viewport + 1;
        }
    }

    pub fn selected_candidate(&self) -> Option<&Entry> {
        self.candidates.get(self.selected)
    }

    /// 获取当前选中的完整路径字符串（用于注入）
    pub fn selected_path(&self) -> Option<String> {
        let entry = self.selected_candidate()?;
        let name = entry.name.to_string_lossy();
        if self.dir_part.is_empty() {
            Some(name.to_string())
        } else {
            Some(format!("{}{}", self.dir_part, name))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    // ===== 保留的原有测试 =====

    #[test]
    fn test_detect_at_sign_with_text() {
        let text = "请看 @main";
        let result = AtMentionState::detect(text, text.len());
        assert!(result.is_some(), "应检测到 @ 提及");
        let (query, pos) = result.unwrap();
        assert_eq!(query, "main");
        assert_eq!(pos, "请看 ".len());
    }

    #[test]
    fn test_detect_no_at_sign() {
        let result = AtMentionState::detect("hello world", "hello world".len());
        assert!(result.is_none(), "无 @ 应返回 None");
    }

    #[test]
    fn test_detect_at_sign_only() {
        let result = AtMentionState::detect("看 @", "看 @".len());
        assert!(result.is_none(), "@ 后无内容应返回 None");
    }

    #[test]
    fn test_detect_path_with_slash() {
        let text = "看 @src/main";
        let result = AtMentionState::detect(text, text.len());
        assert!(result.is_some());
        let (query, _) = result.unwrap();
        assert_eq!(query, "src/main");
    }

    #[test]
    fn test_detect_not_at_line_start() {
        let result = AtMentionState::detect("user@example", "user@example".len());
        assert!(result.is_none(), "非空白前导的 @ 不应触发");
    }

    #[test]
    fn test_move_up_down() {
        let mut state = AtMentionState::new();
        state.active = true;
        state.candidates = vec![
            Entry {
                name: "a.rs".into(),
                is_dir: false,
                is_symlink: false,
            },
            Entry {
                name: "b.rs".into(),
                is_dir: false,
                is_symlink: false,
            },
            Entry {
                name: "c.rs".into(),
                is_dir: false,
                is_symlink: false,
            },
        ];
        assert_eq!(state.selected, 0);
        state.move_down();
        assert_eq!(state.selected, 1);
        state.move_down();
        assert_eq!(state.selected, 2);
        state.move_down();
        assert_eq!(state.selected, 0);
        state.move_up();
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn test_ensure_cwd_sets_once() {
        let mut state = AtMentionState::new();
        assert!(state.cwd.as_os_str().is_empty());
        state.ensure_cwd("/tmp".to_string());
        assert_eq!(state.cwd, PathBuf::from("/tmp"));
        state.ensure_cwd("/other".to_string());
        assert_eq!(state.cwd, PathBuf::from("/tmp"), "ensure_cwd 只设置一次");
    }

    // ===== 新增 dir_cache 测试 =====

    #[test]
    fn test_dir_cache_hit() {
        let mut state = AtMentionState::new();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::write(base.join("test.rs"), "").unwrap();
        state.set_cwd(base.to_string_lossy().to_string());
        state.activate("test".to_string(), 0);

        // 首次刷新：应该 load
        state.refresh_candidates();
        assert!(!state.candidates.is_empty());
        assert!(state.dir_cache.contains_key(base));

        let first_count = state.candidates.len();

        // 新增文件后再次刷新：缓存命中，不应该看到新文件
        fs::write(base.join("new_file.rs"), "").unwrap();
        state.refresh_candidates();
        assert_eq!(
            state.candidates.len(),
            first_count,
            "缓存命中，不应看到新文件"
        );
    }

    #[test]
    fn test_dir_cache_cwd_invalidated() {
        let mut state = AtMentionState::new();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::write(base.join("a.rs"), "").unwrap();
        state.set_cwd(base.to_string_lossy().to_string());
        state.activate("a".to_string(), 0);
        state.refresh_candidates();
        assert!(state.dir_cache.contains_key(base));

        let dir2 = tempfile::tempdir().unwrap();
        state.set_cwd(dir2.path().to_string_lossy().to_string());
        assert!(state.dir_cache.is_empty(), "cwd 变更后缓存应清空");
    }

    #[test]
    fn test_refresh_empty_dir() {
        let mut state = AtMentionState::new();
        let dir = tempfile::tempdir().unwrap();
        state.set_cwd(dir.path().to_string_lossy().to_string());
        state.activate("x".to_string(), 0);
        state.refresh_candidates();
        assert!(state.candidates.is_empty());
        assert_eq!(state.empty_message, Some("(empty directory)".to_string()));
    }

    #[test]
    fn test_refresh_no_match() {
        let mut state = AtMentionState::new();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("readme.md"), "").unwrap();
        state.set_cwd(dir.path().to_string_lossy().to_string());
        state.activate("nonexistent".to_string(), 0);
        state.refresh_candidates();
        assert!(state.candidates.is_empty());
        assert_eq!(state.empty_message, Some("(no matches)".to_string()));
    }
}
