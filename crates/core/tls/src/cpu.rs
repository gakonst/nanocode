use std::io;

#[cfg(target_arch = "x86_64")]
fn missing() -> Vec<&'static str> {
    [
        ("aes", std::arch::is_x86_feature_detected!("aes")),
        ("pclmulqdq", std::arch::is_x86_feature_detected!("pclmulqdq")),
        ("bmi1", std::arch::is_x86_feature_detected!("bmi1")),
        ("bmi2", std::arch::is_x86_feature_detected!("bmi2")),
        ("adx", std::arch::is_x86_feature_detected!("adx")),
        ("avx", std::arch::is_x86_feature_detected!("avx")),
        ("avx2", std::arch::is_x86_feature_detected!("avx2")),
    ]
    .into_iter()
    .filter_map(|(name, present)| (!present).then_some(name))
    .collect()
}

#[cfg(target_arch = "aarch64")]
fn missing() -> Vec<&'static str> {
    [
        ("neon", std::arch::is_aarch64_feature_detected!("neon")),
        ("aes", std::arch::is_aarch64_feature_detected!("aes")),
        ("pmull", std::arch::is_aarch64_feature_detected!("pmull")),
        ("sha2", std::arch::is_aarch64_feature_detected!("sha2")),
    ]
    .into_iter()
    .filter_map(|(name, present)| (!present).then_some(name))
    .collect()
}

pub fn check() -> io::Result<()> {
    let missing = missing();
    if missing.is_empty() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "this CPU lacks {}, which nanocode's TLS cryptography needs",
            missing.join(", ")
        ),
    ))
}
