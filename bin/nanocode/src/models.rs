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
            let (models, note): (Vec<ModelSelection>, _) = match clients.codex_models() {
                Some(Ok(served)) if family == HarnessFamily::Codex => (
                    served.iter().copied().map(ModelSelection::Codex).collect(),
                    None,
                ),
                Some(Err(note)) if family == HarnessFamily::Codex => (Vec::new(), Some(note)),
                _ => (family.models().collect(), None),
            };
            Group {
                title: harness_title(&family),
                note,
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
        for (provider, route, models) in offered {
            groups.push(Group {
                title: format!("Tempo · {provider} · $ per 1M input/output tokens"),
                note: None,
                models: models
                    .into_iter()
                    .map(|(model, about)| (route, model, about))
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
            let known = model.as_codex().is_some_and(|model| !model.is_custom())
                || model.as_claude().is_some();
            let detail = [
                (name != model.as_str()).then(|| name.to_owned()),
                about,
                known.then(|| format!("thinking {thinking}")),
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
    footer.push("/harness picks Codex or Claude".to_owned());
    (
        Picker::new(
            format!("Model · {} harness", harness_title(&current.family())),
            items,
        )
        .filterable()
        .footer(footer.join(" · ")),
        choices,
    )
}

pub fn harness_picker(
    current: &HarnessFamily,
    target: impl Fn(&HarnessFamily) -> String,
) -> Picker {
    let items = HarnessFamily::builtin()
        .into_iter()
        .map(|family| {
            let about = match family == HarnessFamily::Claude {
                true => "Claude Code's agent loop, prompts and tools",
                false => "Codex's agent loop, prompts and tools",
            };
            PickerItem::choice(harness_title(&family))
                .detail(format!("{about} · runs {}", target(&family)))
                .current(family == *current)
        })
        .collect();
    Picker::new("Harness", items).footer("enter select · esc cancel")
}

pub fn harness_at(index: usize) -> Option<HarnessFamily> {
    HarnessFamily::builtin().into_iter().nth(index)
}

fn titled(model: &ModelSelection, route: Route) -> String {
    match route {
        Route::Native => model.display_name().to_owned(),
        route => route.label(model),
    }
}

pub fn thinking_picker(model: &ModelSelection, route: Route, current: Thinking) -> Picker {
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
    Picker::new(format!("Thinking · {}", titled(model, route)), items)
        .footer("enter select · esc cancel")
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

pub const fn thinking_color(thinking: Thinking, theme: &Theme) -> nanocode_tui::ui::style::Color {
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
    route: Route,
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
        format!("Thinking · {}", titled(model, route)),
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
