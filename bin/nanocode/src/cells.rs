const CELL_MEMORY: &str = "NANOCODE_CELL_MEMORY";
const CELL_INTERRUPT_CHECKS: &str = "NANOCODE_CELL_INTERRUPT_CHECKS";

pub fn apply() -> Result<(), String> {
    if let Some(bytes) = setting(CELL_MEMORY)? {
        nanocode::code_mode::set_cell_memory(bytes);
    }
    if let Some(checks) = setting(CELL_INTERRUPT_CHECKS)? {
        nanocode::code_mode::set_interrupt_checks(checks);
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
