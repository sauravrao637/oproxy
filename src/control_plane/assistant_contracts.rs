#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

#[cfg(test)]
use super::assistant_registry::openai_tool_names;
use super::assistant_registry::tool_info;
use super::workspace::{WorkspaceActionRisk, workspace_action_definitions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AssistantToolExecutionKind {
    Read,
    Workspace,
    Proposal,
}

/// Complete risk taxonomy for assistant tools. Not every level is currently
/// assigned to a tool (e.g. `Network`/`Destructive` are reserved for future
/// contracts), so the unused variants are intentional — hence the `dead_code`
/// allow rather than trimming the enum to only the levels in use today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(super) enum AssistantToolRisk {
    Read,
    UiSafe,
    UiSensitive,
    Mutate,
    Network,
    Destructive,
}

#[cfg(test)]
const SUPPORTED_TOOL_RISKS: &[AssistantToolRisk] = &[
    AssistantToolRisk::Read,
    AssistantToolRisk::UiSafe,
    AssistantToolRisk::UiSensitive,
    AssistantToolRisk::Mutate,
    AssistantToolRisk::Network,
    AssistantToolRisk::Destructive,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AssistantToolContract {
    pub(super) name: String,
    pub(super) category: String,
    pub(super) execution_kind: AssistantToolExecutionKind,
    pub(super) requires_confirmation: bool,
    pub(super) risk: AssistantToolRisk,
    pub(super) refreshed_resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AssistantToolContractInfo {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) category: String,
    pub(super) execution_kind: AssistantToolExecutionKind,
    pub(super) requires_confirmation: bool,
    pub(super) risk: AssistantToolRisk,
    pub(super) refreshed_resources: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct StaticToolContract {
    name: &'static str,
    category: &'static str,
    execution_kind: AssistantToolExecutionKind,
    requires_confirmation: bool,
    risk: AssistantToolRisk,
    refreshed_resources: &'static [&'static str],
}

const STATIC_TOOL_CONTRACTS: &[StaticToolContract] = &[
    read_contract("get_feature_catalog"),
    read_contract("list_sessions"),
    read_contract("get_session"),
    read_contract("get_config"),
    read_contract("get_rules"),
    read_contract("get_protocol_metrics"),
    read_contract("get_connections"),
    read_contract("get_breakpoint_diagnostics"),
    read_contract("get_throttling"),
    read_contract("get_dns"),
    read_contract("get_capture_filter"),
    read_contract("get_webhooks"),
    read_contract("get_upstream_proxy"),
    StaticToolContract {
        name: "propose_map_remote",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["rules"],
    },
    StaticToolContract {
        name: "propose_dns_override",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["dns"],
    },
    StaticToolContract {
        name: "propose_throttling",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["throttling"],
    },
    StaticToolContract {
        name: "propose_rewrite_rule",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["rule_sets"],
    },
    StaticToolContract {
        name: "propose_mock_rule",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["mock"],
    },
    StaticToolContract {
        name: "propose_access_rule",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["access"],
    },
    StaticToolContract {
        name: "propose_capture_filter",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["capture_filter"],
    },
    StaticToolContract {
        name: "propose_upstream_proxy",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["upstream_proxy"],
    },
    StaticToolContract {
        name: "propose_action",
        category: "mutate",
        execution_kind: AssistantToolExecutionKind::Proposal,
        requires_confirmation: true,
        risk: AssistantToolRisk::Mutate,
        refreshed_resources: &["workspace", "rules", "sessions", "config"],
    },
];

const fn read_contract(name: &'static str) -> StaticToolContract {
    StaticToolContract {
        name,
        category: "read",
        execution_kind: AssistantToolExecutionKind::Read,
        requires_confirmation: false,
        risk: AssistantToolRisk::Read,
        refreshed_resources: &[],
    }
}

pub(super) fn all_tool_contracts() -> Vec<AssistantToolContract> {
    let mut contracts = static_tool_contracts();
    contracts.extend(workspace_tool_contracts());
    contracts
}

pub(super) fn contract_for_tool(name: &str) -> Option<AssistantToolContract> {
    all_tool_contracts()
        .into_iter()
        .find(|contract| contract.name == name)
}

pub(super) fn grouped_tool_contract_info() -> BTreeMap<String, Vec<AssistantToolContractInfo>> {
    let descriptions: HashMap<String, (String, String)> = tool_info()
        .into_iter()
        .map(|tool| (tool.name, (tool.description, tool.category)))
        .collect();
    let mut grouped: BTreeMap<String, Vec<AssistantToolContractInfo>> = BTreeMap::new();

    for contract in all_tool_contracts() {
        let (description, category) = descriptions
            .get(&contract.name)
            .cloned()
            .unwrap_or_else(|| ("Assistant tool".to_string(), contract.category.clone()));
        let info = AssistantToolContractInfo {
            name: contract.name,
            description,
            category: category.clone(),
            execution_kind: contract.execution_kind,
            requires_confirmation: contract.requires_confirmation,
            risk: contract.risk,
            refreshed_resources: contract.refreshed_resources,
        };
        grouped.entry(category).or_default().push(info);
    }

    for tools in grouped.values_mut() {
        tools.sort_by(|left, right| left.name.cmp(&right.name));
    }

    grouped
}

#[cfg(test)]
pub(super) fn validate_assistant_contracts() -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let openai_names: BTreeSet<String> = openai_tool_names().into_iter().collect();
    let contracts = all_tool_contracts();
    let contract_names: BTreeSet<String> = contracts
        .iter()
        .map(|contract| contract.name.clone())
        .collect();

    for name in &openai_names {
        if !contract_names.contains(name) {
            errors.push(format!("assistant tool '{name}' has no execution contract"));
        }
    }
    for name in &contract_names {
        if !openai_names.contains(name) {
            errors.push(format!(
                "assistant tool contract '{name}' is not exposed to the model"
            ));
        }
    }
    for contract in &contracts {
        if !SUPPORTED_TOOL_RISKS.contains(&contract.risk) {
            errors.push(format!(
                "assistant tool '{}' has an unsupported risk classification",
                contract.name
            ));
        }
        if contract.execution_kind == AssistantToolExecutionKind::Read {
            if contract.requires_confirmation {
                errors.push(format!(
                    "read tool '{}' must not require confirmation",
                    contract.name
                ));
            }
            if contract.risk != AssistantToolRisk::Read {
                errors.push(format!(
                    "read tool '{}' must be classified as read risk",
                    contract.name
                ));
            }
        }
        if contract.execution_kind == AssistantToolExecutionKind::Proposal
            && !contract.requires_confirmation
        {
            errors.push(format!(
                "proposal tool '{}' must require confirmation",
                contract.name
            ));
        }
        if contract.execution_kind == AssistantToolExecutionKind::Workspace
            && contract.requires_confirmation
        {
            errors.push(format!(
                "workspace UI tool '{}' should execute directly without confirmation",
                contract.name
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn static_tool_contracts() -> Vec<AssistantToolContract> {
    STATIC_TOOL_CONTRACTS
        .iter()
        .map(|contract| AssistantToolContract {
            name: contract.name.to_string(),
            category: contract.category.to_string(),
            execution_kind: contract.execution_kind,
            requires_confirmation: contract.requires_confirmation,
            risk: contract.risk,
            refreshed_resources: contract
                .refreshed_resources
                .iter()
                .map(|resource| (*resource).to_string())
                .collect(),
        })
        .collect()
}

fn workspace_tool_contracts() -> Vec<AssistantToolContract> {
    workspace_action_definitions()
        .into_iter()
        .filter_map(|action| {
            let name = action
                .openai_spec
                .pointer("/function/name")
                .and_then(|value| value.as_str())?
                .to_string();
            Some(AssistantToolContract {
                name,
                category: action.category,
                execution_kind: AssistantToolExecutionKind::Workspace,
                requires_confirmation: false,
                risk: workspace_risk(action.risk),
                refreshed_resources: action.refreshed_resources,
            })
        })
        .collect()
}

fn workspace_risk(risk: WorkspaceActionRisk) -> AssistantToolRisk {
    match risk {
        WorkspaceActionRisk::UiSafe => AssistantToolRisk::UiSafe,
        WorkspaceActionRisk::UiSensitive => AssistantToolRisk::UiSensitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_tool_has_a_contract() {
        validate_assistant_contracts().expect("assistant contracts should match exposed tools");
    }

    #[test]
    fn read_tools_are_automatic_and_read_risk() {
        let contract = contract_for_tool("list_sessions").expect("list sessions contract");

        assert_eq!(contract.execution_kind, AssistantToolExecutionKind::Read);
        assert_eq!(contract.risk, AssistantToolRisk::Read);
        assert!(!contract.requires_confirmation);
    }

    #[test]
    fn proposal_tools_require_confirmation() {
        for name in [
            "propose_map_remote",
            "propose_dns_override",
            "propose_throttling",
            "propose_rewrite_rule",
            "propose_mock_rule",
            "propose_access_rule",
            "propose_capture_filter",
            "propose_upstream_proxy",
            "propose_action",
        ] {
            let contract = contract_for_tool(name).expect("proposal contract");

            assert_eq!(
                contract.execution_kind,
                AssistantToolExecutionKind::Proposal
            );
            assert!(contract.requires_confirmation);
        }
    }

    #[test]
    fn workspace_contracts_are_derived_from_workspace_actions() {
        let contract = contract_for_tool("workspace_sessions_apply_filter")
            .expect("workspace filter contract");

        assert_eq!(
            contract.execution_kind,
            AssistantToolExecutionKind::Workspace
        );
        assert_eq!(contract.category, "ui");
        assert!(!contract.requires_confirmation);
        assert!(
            contract
                .refreshed_resources
                .iter()
                .any(|item| item == "sessions")
        );
    }

    #[test]
    fn grouped_tool_contract_info_enriches_read_tools() {
        let grouped = grouped_tool_contract_info();
        let read_tools = grouped.get("read").expect("read tools");
        let list_sessions = read_tools
            .iter()
            .find(|tool| tool.name == "list_sessions")
            .expect("list sessions metadata");

        assert_eq!(
            list_sessions.execution_kind,
            AssistantToolExecutionKind::Read
        );
        assert_eq!(list_sessions.risk, AssistantToolRisk::Read);
        assert!(!list_sessions.requires_confirmation);
        assert!(list_sessions.description.contains("captured sessions"));
    }

    #[test]
    fn grouped_tool_contract_info_enriches_proposal_and_workspace_tools() {
        let grouped = grouped_tool_contract_info();
        let mutate_tools = grouped.get("mutate").expect("mutate tools");
        let proposal = mutate_tools
            .iter()
            .find(|tool| tool.name == "propose_action")
            .expect("proposal metadata");
        assert_eq!(
            proposal.execution_kind,
            AssistantToolExecutionKind::Proposal
        );
        assert_eq!(proposal.risk, AssistantToolRisk::Mutate);
        assert!(proposal.requires_confirmation);

        let ui_tools = grouped.get("ui").expect("ui tools");
        let filter = ui_tools
            .iter()
            .find(|tool| tool.name == "workspace_sessions_apply_filter")
            .expect("workspace filter metadata");
        assert_eq!(filter.execution_kind, AssistantToolExecutionKind::Workspace);
        assert_eq!(filter.risk, AssistantToolRisk::UiSafe);
        assert!(!filter.requires_confirmation);
        assert!(
            filter
                .refreshed_resources
                .iter()
                .any(|item| item == "sessions")
        );
    }
}
