use nanocode::{HarnessFamily, ModelSelection, ModelTransport, ServiceTier, Thinking};
use nanocode_tui::{
    effort::{EffortDial, Level},
    picker::{ItemKind, Picker, PickerItem},
    theme::Theme,
};

pub fn catalog(clients: &crate::host::ModelClients) -> Vec<(HarnessFamily, Vec<ModelSelection>)> {
    HarnessFamily::builtin()
        .into_iter()
        .map(|family| {
            let models = match clients.codex_models() {
                Some(served) if family == HarnessFamily::Codex => {
                    served.iter().copied().map(ModelSelection::Codex).collect()
                }
                _ => family.models().collect(),
            };
            (family, models)
        })
        .collect()
}

fn harness_title(family: &HarnessFamily) -> String {
    if *family == HarnessFamily::Claude {
        "Claude".into()
    } else {
        "Codex".into()
    }
}

pub fn features(model: &ModelSelection) -> String {
    let capabilities = model.capabilities(ModelTransport::Native);
    let mut features = Vec::new();
    for tier in capabilities
        .service_tiers()
        .filter(|tier| *tier != ServiceTier::Standard)
    {
        features.push(tier.as_str());
    }
    features.join(" · ")
}

pub fn model_picker(
    current: &ModelSelection,
    gateway: Option<&crate::host::Gateway>,
    clients: &crate::host::ModelClients,
) -> Picker {
    let mut items = Vec::new();
    for (family, models) in catalog(clients) {
        items.push(PickerItem::header(harness_title(&family)));
        for model in models {
            if gateway.is_some_and(|gateway| !gateway.serves(&model)) {
                continue;
            }
            let capabilities = model.capabilities(ModelTransport::Native);
            let thinking = capabilities
                .thinking()
                .map(Thinking::as_str)
                .collect::<Vec<_>>()
                .join("/");
            let mut item = PickerItem::choice(model.to_string())
                .detail(format!("{} · thinking {thinking}", model.display_name()))
                .current(model == *current);
            let features = features(&model);
            if !features.is_empty() {
                item = item.badge(features);
            }
            items.push(item);
        }
    }
    Picker::new("Model", items)
        .filterable()
        .footer("type to filter · enter select · esc cancel")
}

pub fn model_at(
    picker: &Picker,
    index: usize,
    clients: &crate::host::ModelClients,
) -> Option<ModelSelection> {
    let item = picker.items().get(index)?;
    (item.kind == ItemKind::Choice).then(|| clients.parse_model(&item.label).ok())?
}

pub fn thinking_picker(model: &ModelSelection, current: Thinking) -> Picker {
    let capabilities = model.capabilities(ModelTransport::Native);
    let items = capabilities
        .thinking()
        .map(|thinking| {
            let mut item = PickerItem::choice(thinking.as_str()).current(thinking == current);
            if thinking == capabilities.default_thinking() {
                item = item.detail("default");
            }
            item
        })
        .collect();
    Picker::new(format!("Thinking · {model}"), items).footer("enter select · esc cancel")
}

pub fn thinking_at(model: &ModelSelection, picker: &Picker, index: usize) -> Option<Thinking> {
    let label = &picker.items().get(index)?.label;
    model
        .capabilities(ModelTransport::Native)
        .thinking()
        .find(|thinking| thinking.as_str() == label)
}

pub fn tier_options(model: &ModelSelection) -> Vec<ServiceTier> {
    model
        .capabilities(ModelTransport::Native)
        .service_tiers()
        .collect()
}

pub fn tier_picker(model: &ModelSelection, tiers: &[ServiceTier], current: ServiceTier) -> Picker {
    let items = tiers
        .iter()
        .map(|tier| {
            PickerItem::choice(tier.as_str())
                .detail(match tier {
                    ServiceTier::Standard => "default speed",
                    ServiceTier::Fast | ServiceTier::Priority => "priority processing",
                    ServiceTier::Ultrafast => "lowest latency",
                })
                .current(*tier == current)
        })
        .collect();
    Picker::new(format!("Speed · {model}"), items).footer("enter select · esc cancel")
}

pub const fn thinking_color(
    thinking: Thinking,
    theme: &Theme,
) -> nanocode_tui::ratatui::style::Color {
    match thinking {
        Thinking::None => theme.muted(),
        Thinking::Low => theme.thinking_low(),
        Thinking::Medium => theme.thinking_medium(),
        Thinking::High => theme.thinking_high(),
        Thinking::Xhigh => theme.thinking_xhigh(),
        Thinking::Max => theme.thinking_max(),
    }
}

pub fn thinking_dial(
    model: &ModelSelection,
    current: Thinking,
    theme: &Theme,
) -> Option<(EffortDial, Vec<Thinking>)> {
    let levels = model
        .capabilities(ModelTransport::Native)
        .thinking()
        .collect::<Vec<_>>();
    let selected = levels
        .iter()
        .position(|level| *level == current)
        .unwrap_or(0);
    let dial = EffortDial::new(
        format!("Thinking · {}", model.display_name()),
        levels
            .iter()
            .map(|level| Level {
                label: level.as_str().to_owned(),
                color: thinking_color(*level, theme),
            })
            .collect(),
        selected,
    )?;
    Some((dial, levels))
}
