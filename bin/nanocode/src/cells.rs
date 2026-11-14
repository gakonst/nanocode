const CELL_MEMORY: &str = "NANOCODE_CELL_MEMORY";

pub fn apply() -> Result<(), String> {
    if let Some(bytes) = setting(CELL_MEMORY)? {
        nanocode::code_mode::set_cell_memory(bytes);
    }
    Ok(())
}

fn setting<T: std::str::FromStr>(name: &str) -> Result<Option<T>, String> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    value
        .to_str()
        .and_then(|value| value.trim().parse().ok())
        .map(Some)
        .ok_or_else(|| format!("{name} must be a whole number, not {value:?}"))
}
