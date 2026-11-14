use nanocode::Tools;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[nanocode::tool(description = "Add two signed integers, reporting overflow")]
async fn checked_sum(a: i64, b: i64) -> std::io::Result<i64> {
    a.checked_add(b)
        .ok_or_else(|| std::io::Error::other("integer addition overflow"))
}

#[nanocode::tool(description = "Divide signed integers, reporting zero divisors and overflow")]
async fn quotient(a: i64, b: i64) -> std::io::Result<i64> {
    a.checked_div(b)
        .ok_or_else(|| std::io::Error::other("division by zero or integer division overflow"))
}

pub fn tools() -> Result<Tools> {
    Ok(Tools::builder()
        .without_defaults()
        .tool(checked_sum)
        .tool(quotient)
        .build()?)
}

pub fn main(run: impl Future<Output = Result<()>>) {
    let outcome = nanocode_executor::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.into())
        .and_then(|runtime| runtime.block_on(run));
    if let Err(error) = outcome {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
