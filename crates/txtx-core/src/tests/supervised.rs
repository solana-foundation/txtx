use std::{thread, time::Duration};

use tokio::sync::broadcast;
use txtx_addon_kit::{
    channel::{self, Receiver},
    futures::executor::block_on,
    helpers::fs::FileLocation,
    indexmap::IndexMap,
    types::{
        cloud_interface::CloudServiceContext,
        frontend::{
            ActionItemRequest, ActionItemRequestType, ActionItemResponse, ActionItemResponseType,
            ActionItemStatus, ActionPanelData, BlockEvent, ReviewedInputResponse,
        },
        AuthorizationContext, RunbookId,
    },
};

use crate::{
    constants::{ACTION_ITEM_ENV, ACTION_ITEM_GENESIS, ACTION_ITEM_VALIDATE_BLOCK},
    runbook::{Runbook, RunbookExecutionMode, RunbookSources, RunbookTopLevelInputsMap},
    start_supervised_runbook_runloop,
};

fn build_runbook(source: &str) -> Runbook {
    let mut sources = RunbookSources::new();
    sources.add_source(
        "supervised.tx".into(),
        FileLocation::from_path_string(".").unwrap(),
        source.into(),
    );
    let environments = IndexMap::from([
        ("devnet".into(), IndexMap::from([("amount".into(), "1".into())])),
        ("testnet".into(), IndexMap::from([("amount".into(), "2".into())])),
    ]);
    let inputs =
        RunbookTopLevelInputsMap::from_environment_map(&Some("devnet".into()), &environments);
    let mut runbook = Runbook::new(RunbookId::zero(), None);
    block_on(runbook.build_contexts_from_sources(
        sources,
        inputs,
        AuthorizationContext::empty(),
        super::get_addon_by_namespace,
        CloudServiceContext::empty(),
    ))
    .unwrap();
    runbook.enable_full_execution_mode();
    runbook
}

fn with_supervised_run(
    mut runbook: Runbook,
    interact: impl FnOnce(&Receiver<BlockEvent>, &broadcast::Sender<ActionItemResponse>),
) -> Runbook {
    thread::scope(|scope| {
        let (events_tx, events_rx) = channel::unbounded();
        let (responses_tx, responses_rx) = broadcast::channel(32);
        let runloop = scope.spawn(move || {
            block_on(start_supervised_runbook_runloop(&mut runbook, events_tx, responses_rx))
                .unwrap();
            runbook
        });
        interact(&events_rx, &responses_tx);
        drop(responses_tx);
        runloop.join().unwrap()
    })
}

fn receive(events: &Receiver<BlockEvent>) -> BlockEvent {
    events.recv_timeout(Duration::from_secs(5)).expect("supervisor did not emit the expected event")
}

fn expect_panel(events: &Receiver<BlockEvent>, title: &str) -> ActionPanelData {
    let event = receive(events);
    let panel = event.expect_block().panel.expect_action_panel();
    assert_eq!(panel.title, title);
    panel.clone()
}

fn request(panel: &ActionPanelData, key: &str) -> ActionItemRequest {
    panel
        .groups
        .iter()
        .flat_map(|group| &group.sub_groups)
        .flat_map(|subgroup| &subgroup.action_items)
        .find(|item| item.internal_key == key)
        .unwrap_or_else(|| panic!("missing {key} action in {}", panel.title))
        .clone()
}

fn expect_checklist(events: &Receiver<BlockEvent>, index: usize, name: &str) -> ActionPanelData {
    let panel = expect_panel(events, "Flow Execution");
    let ActionItemRequestType::BeginFlow(flow) =
        &panel.groups[0].sub_groups[0].action_items[0].action_type
    else {
        panic!("expected a flow announcement");
    };
    let flow = serde_json::to_value(flow).unwrap();
    assert_eq!(flow["index"], index);
    assert_eq!(flow["name"], name);
    let checklist = expect_panel(events, "runbook checklist");
    request(&checklist, ACTION_ITEM_GENESIS);
    checklist
}

fn respond(
    responses: &broadcast::Sender<ActionItemResponse>,
    request: &ActionItemRequest,
    payload: ActionItemResponseType,
) {
    responses.send(ActionItemResponse { action_item_id: request.id.clone(), payload }).unwrap();
}

fn validate(
    events: &Receiver<BlockEvent>,
    responses: &broadcast::Sender<ActionItemResponse>,
    request: &ActionItemRequest,
) {
    respond(responses, request, ActionItemResponseType::ValidateBlock);
    assert!(matches!(receive(events), BlockEvent::UpdateActionItems(_)));
}

fn approve_variable(
    events: &Receiver<BlockEvent>,
    responses: &broadcast::Sender<ActionItemResponse>,
    panel: &ActionPanelData,
) {
    let review = request(panel, "check_input");
    assert_eq!(review.action_status, ActionItemStatus::Todo);
    respond(
        responses,
        &review,
        ActionItemResponseType::ReviewInput(ReviewedInputResponse {
            input_name: "value".into(),
            value_checked: true,
            force_execution: false,
        }),
    );
    assert!(matches!(receive(events), BlockEvent::UpdateActionItems(_)));
}

#[test]
fn supervised_execution_skips_ignored_flows_and_completes_after_the_last_enabled_flow() {
    let mut runbook = build_runbook(
        r#"
        flow "ignored_before" {}
        flow "first" {}
        flow "ignored_between" {}
        flow "second" {}
        flow "ignored_after" {}
        output "amount" {
            value = input.amount
        }
        "#,
    );
    for index in [0, 2, 4] {
        runbook.flow_contexts[index].execution_context.execution_mode =
            RunbookExecutionMode::Ignored;
    }
    with_supervised_run(runbook, |events, responses| {
        for (index, name) in [(1, "first"), (3, "second")] {
            let checklist = expect_checklist(events, index, name);
            validate(events, responses, &request(&checklist, ACTION_ITEM_GENESIS));
            expect_panel(events, "output review");
        }
        assert!(matches!(receive(events), BlockEvent::RunbookCompleted(_)));
    });
}

#[test]
fn supervised_execution_completes_without_a_response_when_all_flows_are_ignored() {
    let mut runbook = build_runbook("flow \"first\" {}\nflow \"second\" {}");
    for flow in &mut runbook.flow_contexts {
        flow.execution_context.execution_mode = RunbookExecutionMode::Ignored;
    }

    with_supervised_run(runbook, |events, _responses| {
        assert!(matches!(receive(events), BlockEvent::RunbookCompleted(_)));
    });
}

#[test_case::test_case(0; "from_first_flow")]
#[test_case::test_case(1; "after_skipping_first_flow")]
fn switching_environment_restarts_flows_and_requires_fresh_approvals(initial_flow_index: usize) {
    let mut runbook = build_runbook(
        r#"
        flow "first" {}
        flow "second" {}
        variable "amount" {
            value = input.amount
        }
        output "amount" {
            value = variable.amount
        }
        "#,
    );
    if initial_flow_index == 1 {
        runbook.flow_contexts[0].execution_context.execution_mode = RunbookExecutionMode::Ignored;
    }

    let runbook = with_supervised_run(runbook, |events, responses| {
        let name = ["first", "second"][initial_flow_index];
        let checklist = expect_checklist(events, initial_flow_index, name);
        validate(events, responses, &request(&checklist, ACTION_ITEM_GENESIS));
        let variables = expect_panel(events, "Variables Review");
        approve_variable(events, responses, &variables);

        // Earlier flows and old approvals must not carry over to a new environment.
        respond(
            responses,
            &request(&checklist, ACTION_ITEM_ENV),
            ActionItemResponseType::PickInputOption("testnet".into()),
        );
        assert!(matches!(receive(events), BlockEvent::Clear));
        for (index, name) in [(0, "first"), (1, "second")] {
            let checklist = expect_checklist(events, index, name);
            let ActionItemRequestType::PickInputOption(environment) =
                request(&checklist, ACTION_ITEM_ENV).action_type
            else {
                panic!("expected an environment selector");
            };
            assert_eq!(environment.selected.value, "testnet");
            validate(events, responses, &request(&checklist, ACTION_ITEM_GENESIS));
            let variables = expect_panel(events, "Variables Review");
            approve_variable(events, responses, &variables);
            validate(events, responses, &request(&variables, ACTION_ITEM_VALIDATE_BLOCK));
            expect_panel(events, "output review");
        }
        assert!(matches!(receive(events), BlockEvent::RunbookCompleted(_)));
    });
    assert_eq!(runbook.get_active_inputs_selector().as_deref(), Some("testnet"));
    assert!(runbook.flow_contexts.iter().all(|flow| flow.is_enabled()));
}
