use nanocode::{ModelSelection, ModelTransport, ServiceTier, Thinking};
use nanocode_tui::{
    effort::{EffortDial, Level},
    picker::{Picker, PickerItem},
    theme::Theme,
};

use crate::gateways::Routes;

pub type Choices = Vec<Option<(usize, ModelSelection)>>;

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
    gateway: Option<usize>,
    routes: &Routes,
) -> (Picker, Choices) {
    let mut items = Vec::new();
    let mut choices = Vec::new();
    let mut footer = vec!["type to filter · enter select · esc cancel".to_owned()];
    for (index, offered) in routes.gateways().iter().enumerate() {
        let (models, note) = offered.models();
        if let Some(note) = note {
            footer.push(format!("{}: {note}", offered.name));
        }
        if models.is_empty() {
            continue;
        }
        items.push(PickerItem::header(offered.title()));
        choices.push(None);
        for model in models {
            let capabilities = model.capabilities(ModelTransport::Native);
            let thinking = capabilities
                .thinking()
                .map(Thinking::as_str)
                .collect::<Vec<_>>()
                .join("/");
            let label = model.to_string();
            let detail = match model.display_name() {
                name if name == label => format!("thinking {thinking}"),
                name => format!("{name} · thinking {thinking}"),
            };
            let mut item = PickerItem::choice(label)
                .detail(detail)
                .current(model == *current && gateway == Some(index));
            let features = features(&model);
            if !features.is_empty() {
                item = item.badge(features);
            }
            items.push(item);
            choices.push(Some((index, model)));
        }
    }
    (
        Picker::new("Model", items)
            .filterable()
            .footer(footer.join(" · ")),
        choices,
    )
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
