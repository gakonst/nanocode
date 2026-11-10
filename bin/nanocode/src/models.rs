use nanocode::{HarnessFamily, ModelSelection, ModelTransport, ServiceTier, Thinking};
use nanocode_tui::{
    effort::{EffortDial, Level},
    picker::{Picker, PickerItem},
    theme::Theme,
};

use crate::{host::Route, tempo::Tempo};

pub type Choices = Vec<Option<(Route, ModelSelection)>>;

pub struct Group {
    pub title: String,
    pub note: Option<String>,
    pub models: Vec<(Route, ModelSelection, Option<String>)>,
}

pub fn catalog(clients: &crate::host::ModelClients) -> Vec<Group> {
    let mut groups = HarnessFamily::builtin()
        .into_iter()
        .map(|family| {
            let models: Vec<ModelSelection> = match clients.codex_models() {
                Some(served) if family == HarnessFamily::Codex => {
                    served.iter().copied().map(ModelSelection::Codex).collect()
                }
                _ => family.models().collect(),
            };
            Group {
                title: harness_title(&family),
                note: None,
                models: models
                    .into_iter()
                    .map(|model| (Route::Native, model, None))
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    if let Some(tempo) = clients.tempo_models() {
        let (offered, note) = tempo.groups();
        let note = note.map(|note| format!("Tempo: {note}"));
        if offered.is_empty() {
            groups.push(Group {
                title: format!("Tempo · {}", Tempo::ABOUT),
                note,
                models: Vec::new(),
            });
        }
        for (prefix, models) in offered {
            groups.push(Group {
                title: match prefix {
                    Some(prefix) => format!("Tempo · {}{prefix}/", Tempo::PREFIX),
                    None => "Tempo · Codex and Claude models through the Tempo gateway, paid from the Tempo wallet".to_owned(),
                },
                note: None,
                models: models
                    .into_iter()
                    .map(|(model, about)| (Route::Tempo, model, about))
                    .collect(),
            });
        }
    }
    groups
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
    route: Route,
    gateway: Option<&crate::host::Gateway>,
    clients: &crate::host::ModelClients,
) -> (Picker, Choices) {
    let mut items = Vec::new();
    let mut choices = Vec::new();
    let mut footer = vec!["type to filter · enter select · esc cancel".to_owned()];
    for group in catalog(clients) {
        footer.extend(group.note);
        let models = group
            .models
            .into_iter()
            .filter(|(offered, model, _)| {
                *offered != Route::Native || gateway.is_none_or(|gateway| gateway.serves(model))
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            continue;
        }
        items.push(PickerItem::header(group.title));
        choices.push(None);
        for (offered, model, about) in models {
            let capabilities = model.capabilities(ModelTransport::Native);
            let thinking = capabilities
                .thinking()
                .map(Thinking::as_str)
                .collect::<Vec<_>>()
                .join("/");
            let name = model.display_name();
            let detail = [
                (name != model.as_str()).then(|| name.to_owned()),
                about,
                Some(format!("thinking {thinking}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let mut item = PickerItem::choice(offered.label(&model))
                .detail(detail)
                .current(model == *current && offered == route);
            let features = features(&model);
            if !features.is_empty() {
                item = item.badge(features);
            }
            items.push(item);
            choices.push(Some((offered, model)));
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
