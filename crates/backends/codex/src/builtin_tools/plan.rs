use nanocode_sync::SwapOption;
use nanocode_tools::ToolDefinition;
use serde::Deserialize;
use serde_json::json;

use super::{StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolResult};

pub struct UpdatePlanTool {
    current: SwapOption<UpdatePlanArgs>,
}

impl UpdatePlanTool {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current: SwapOption::const_empty(),
        }
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for UpdatePlanTool {
    fn definition(&self) -> ToolDefinition {
        StandardTool::UpdatePlan.definition()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> nanocode_tools::ToolCallFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let plan = input.decode_json::<UpdatePlanArgs>()?;
                tracing::debug!(
                    explanation = ?plan.explanation,
                    step_count = plan.plan.len(),
                    "updating plan"
                );
                for (index, item) in plan.plan.iter().enumerate() {
                    tracing::debug!(
                        index,
                        step = item.step,
                        status = ?item.status,
                        "updated plan item"
                    );
                }
                self.current.store(Some(std::sync::Arc::new(plan)));
                Ok(ToolOutput::text("Plan updated").with_structured_result(json!({})))
            };
            output
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePlanArgs {
    #[serde(default)]
    explanation: Option<String>,
    plan: Vec<PlanItem>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanItem {
    step: String,
    status: PlanStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}
