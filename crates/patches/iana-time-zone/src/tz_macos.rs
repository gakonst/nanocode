pub(crate) fn get_timezone_inner() -> Result<String, crate::GetTimezoneError> {
    let link = std::fs::read_link("/etc/localtime")?;
    let link = link
        .to_str()
        .ok_or(crate::GetTimezoneError::FailedParsingString)?;
    link.split_once("/zoneinfo/")
        .map(|(_, name)| name.to_owned())
        .ok_or(crate::GetTimezoneError::FailedParsingString)
}
