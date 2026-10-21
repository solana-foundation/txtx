use txtx_addon_kit::types::{
    frontend::{
        ActionItemResponse, ActionItemResponseType, ActionItemStatus, ProvidedInputResponse,
        ReviewedInputResponse,
    },
    types::Value,
};
use txtx_addon_kit::{types::block_id::BlockId, Addon};
use txtx_test_utils::test_harness::setup_test;

use crate::std::StdAddon;

#[test]
fn switching_environment_keeps_the_current_flow_ready_to_start() {
    use std::collections::BTreeMap;

    use txtx_addon_kit::helpers::fs::FileLocation;
    use txtx_addon_kit::types::cloud_interface::CloudServiceContext;
    use txtx_addon_kit::types::frontend::{ActionItemRequestType, BlockEvent};
    use txtx_addon_kit::types::{AuthorizationContext, RunbookId};
    use txtx_addon_kit::{channel, futures::executor::block_on, indexmap::IndexMap};

    use crate::runbook::{Runbook, RunbookSources, RunbookTopLevelInputsMap};

    block_on(async {
        let mut sources = RunbookSources::new();
        sources.add_source(
            "environments.tx".into(),
            FileLocation::from_path_string(".").unwrap(),
            "flow \"first\" {}\nflow \"second\" {}".into(),
        );
        let environments = IndexMap::from([
            ("devnet".to_string(), IndexMap::new()),
            ("testnet".to_string(), IndexMap::new()),
        ]);
        let inputs =
            RunbookTopLevelInputsMap::from_environment_map(&Some("devnet".into()), &environments);
        let mut runbook = Runbook::new(RunbookId::zero(), None);
        runbook
            .build_contexts_from_sources(
                sources,
                inputs,
                AuthorizationContext::empty(),
                get_addon_by_namespace,
                CloudServiceContext::empty(),
            )
            .await
            .unwrap();
        runbook.enable_full_execution_mode();
        let (progress_tx, progress_rx) = channel::unbounded();
        let mut requests = BTreeMap::new();

        // Rebuilding for another environment previously cleared the UI without a new checklist.
        crate::reset_runbook_execution(
            &mut runbook,
            &ActionItemResponseType::PickInputOption("testnet".into()),
            &mut requests,
            &BTreeMap::new(),
            &progress_tx,
            1,
            2,
        )
        .await
        .unwrap();

        assert_eq!(runbook.get_active_inputs_selector().as_deref(), Some("testnet"));
        assert!(runbook.flow_contexts.iter().all(|flow| flow.is_enabled()));
        assert!(matches!(progress_rx.try_recv().unwrap(), BlockEvent::Clear));
        let flow_event = progress_rx.try_recv().unwrap();
        let flow_panel = flow_event.expect_block().panel.expect_action_panel();
        assert_eq!(flow_panel.title, "Flow Execution");
        let checklist_event = progress_rx.try_recv().unwrap();
        let checklist = checklist_event.expect_block().panel.expect_action_panel();
        assert_eq!(checklist.title, "runbook checklist");
        let flow = requests
            .values()
            .find_map(|request| match &request.action_type {
                ActionItemRequestType::BeginFlow(flow) => Some(serde_json::to_value(flow).unwrap()),
                _ => None,
            })
            .unwrap();
        assert_eq!(flow["index"], 1);
        assert_eq!(flow["name"], "second");
        assert!(requests
            .values()
            .any(|request| { request.internal_key == crate::constants::ACTION_ITEM_GENESIS }));
    });
}

pub fn get_addon_by_namespace(namespace: &str) -> Option<Box<dyn Addon>> {
    let available_addons: Vec<Box<dyn Addon>> = vec![Box::new(StdAddon::new())];
    for addon in available_addons.into_iter() {
        if namespace.starts_with(&format!("{}", addon.get_namespace())) {
            return Some(addon);
        }
    }
    None
}

#[test]
fn test_ab_c_runbook_no_env() {
    // Load Runbook ab_c.tx
    let abc_tx = include_str!("./fixtures/ab_c.tx");

    let harness = setup_test("ab_c.tx", &abc_tx, get_addon_by_namespace);

    // runbook checklist assertions
    {
        let action_panel_data =
            harness.expect_action_panel(None, "runbook checklist", vec![vec![1]]);

        let validate_button = &action_panel_data.groups[0].sub_groups[0].action_items[0];

        let start_runbook = &action_panel_data.groups[0].sub_groups[0].action_items[0];
        // assert_eq!(start_runbook.action_status, ActionItemStatus::Success(None));
        assert_eq!(start_runbook.construct_instance_name.to_uppercase(), "START RUNBOOK");

        // Complete start_runbook action
        harness.send_and_expect_action_item_update(
            ActionItemResponse {
                action_item_id: start_runbook.id.clone(),
                payload: ActionItemResponseType::ValidateBlock,
            },
            vec![(&validate_button.id, Some(ActionItemStatus::Success(None)))],
        );
    }
    // Review inputs assertions
    {
        let inputs_panel_data =
            harness.expect_action_panel(None, "variables review", vec![vec![2, 1]]);

        let input_a_action = &inputs_panel_data.groups[0].sub_groups[0].action_items[0];
        let input_b_action = &inputs_panel_data.groups[0].sub_groups[0].action_items[1];

        assert_eq!(&input_a_action.internal_key, "check_input");
        assert_eq!(&input_b_action.internal_key, "provide_input");

        // review input a and expect action item update
        harness.send_and_expect_action_item_update(
            ActionItemResponse {
                action_item_id: input_a_action.id.clone(),
                payload: ActionItemResponseType::ReviewInput(ReviewedInputResponse {
                    value_checked: true,
                    input_name: "value".into(),
                    force_execution: false,
                }),
            },
            vec![(&input_a_action.id, Some(ActionItemStatus::Success(None)))],
        );

        // Should be a no-op
        harness.expect_noop();

        // provide input b and expect no update
        harness.send(&ActionItemResponse {
            action_item_id: input_b_action.id.clone(),
            payload: ActionItemResponseType::ProvideInput(ProvidedInputResponse {
                updated_value: Value::integer(5),
                input_name: "value".into(),
            }),
        });
        // review input b and expect action item update
        harness.send_and_expect_action_item_update(
            ActionItemResponse {
                action_item_id: input_b_action.id.clone(),
                payload: ActionItemResponseType::ReviewInput(ReviewedInputResponse {
                    value_checked: true,
                    input_name: "value".into(),
                    force_execution: false,
                }),
            },
            vec![(&input_b_action.id, Some(ActionItemStatus::Success(None)))],
        );

        // our validate block button yields another action item update for input b, but it would be filtered
        // out from being propagated to the frontend... we should probably update tests to check this
        harness.send_and_expect_action_item_update(
            ActionItemResponse {
                action_item_id: BlockId::new(&vec![]),
                payload: ActionItemResponseType::ValidateBlock,
            },
            vec![(&input_b_action.id, Some(ActionItemStatus::Success(None)))],
        );
    }

    // assert output review
    {
        harness.expect_action_panel(None, "output review", vec![vec![1]]);
    }

    harness.expect_runbook_complete();
}
