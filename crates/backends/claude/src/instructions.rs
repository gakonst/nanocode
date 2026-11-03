const CODE_MODE: &str = include_str!("../prompts/code_mode.md");

pub(crate) fn default_system_blocks(code_only: bool) -> std::sync::Arc<Vec<serde_json::Value>> {
    static PLAIN: std::sync::OnceLock<std::sync::Arc<Vec<serde_json::Value>>> =
        std::sync::OnceLock::new();
    static CODE: std::sync::OnceLock<std::sync::Arc<Vec<serde_json::Value>>> =
        std::sync::OnceLock::new();
    let blocks = if code_only { &CODE } else { &PLAIN };
    std::sync::Arc::clone(blocks.get_or_init(|| {
        let mut blocks = crate::upstream::system_blocks();
        if code_only {
            blocks.push(serde_json::json!({"type": "text", "text": CODE_MODE}));
        }
        std::sync::Arc::new(blocks)
    }))
}
