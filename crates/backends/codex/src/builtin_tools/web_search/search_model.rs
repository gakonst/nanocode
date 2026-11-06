use crate::client::Model;

pub(super) fn search_model(model: &str, fallback: Model) -> &str {
    let other_vendor = match Model::known(model) {
        Some(Model::Glm53 | Model::Kimi | Model::Mimo) => true,
        Some(_) => false,
        None => model
            .split_once('/')
            .is_some_and(|(vendor, _)| vendor != "openai"),
    };
    if other_vendor {
        fallback.as_str()
    } else {
        model
    }
}
