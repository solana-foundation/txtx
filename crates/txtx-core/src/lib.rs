#[macro_use]
extern crate lazy_static;

#[macro_use]
pub extern crate txtx_addon_kit as kit;

pub extern crate mustache;

mod constants;
pub mod errors;
pub mod eval;
pub mod manifest;
// pub mod snapshot;
pub mod runbook;
pub mod std;
pub mod templates;
pub mod types;
pub mod validation;

#[cfg(test)]
mod tests;
pub mod utils;

use ::std::collections::BTreeMap;
use ::std::future::Future;
use ::std::pin::Pin;
use ::std::thread::sleep;
use ::std::time::Duration;

use crate::runbook::flow_context::FlowContext;
use constants::ACTION_ITEM_ENV;
use constants::ACTION_ITEM_GENESIS;
use constants::ACTION_ITEM_VALIDATE_BLOCK;
use eval::run_constructs_evaluation;
use eval::run_signers_evaluation;
use kit::constants::ACTION_ITEM_CHECK_BALANCE;
use runbook::get_source_context_for_diagnostic;
use tokio::sync::broadcast::error::TryRecvError;
use txtx_addon_kit::channel::Sender;
use txtx_addon_kit::constants::ACTION_ITEM_CHECK_ADDRESS;
use txtx_addon_kit::hcl::Span;
use txtx_addon_kit::types::block_id::BlockId;
use txtx_addon_kit::types::commands::CommandExecutionResult;
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::frontend::ActionItemRequest;
use txtx_addon_kit::types::frontend::ActionItemRequestType;
use txtx_addon_kit::types::frontend::ActionItemRequestUpdate;
use txtx_addon_kit::types::frontend::ActionItemResponse;
use txtx_addon_kit::types::frontend::ActionItemResponseType;
use txtx_addon_kit::types::frontend::ActionItemStatus;
use txtx_addon_kit::types::frontend::Actions;
use txtx_addon_kit::types::frontend::Block;
use txtx_addon_kit::types::frontend::BlockEvent;
use txtx_addon_kit::types::frontend::ErrorPanelData;
use txtx_addon_kit::types::frontend::InputOption;
use txtx_addon_kit::types::frontend::NormalizedActionItemRequestUpdate;
use txtx_addon_kit::types::frontend::Panel;
use txtx_addon_kit::types::frontend::PickInputOptionRequest;
use txtx_addon_kit::types::frontend::ReviewedInputResponse;
use txtx_addon_kit::types::frontend::ValidateBlockData;
use txtx_addon_kit::types::types::RunbookSupervisionContext;
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;
use types::Runbook;

lazy_static! {
    // create this action so we can reference its `id` property, which is built from the immutable data
     pub static ref SET_ENV_ACTION: ActionItemRequest =ActionItemRequestType::PickInputOption(PickInputOptionRequest {
            options: vec![],
            selected: InputOption::default(),
        }).to_request("", ACTION_ITEM_ENV)
        .with_meta_description("Select the environment to target")
        .with_status(ActionItemStatus::Success(None))
      ;
}

pub async fn start_unsupervised_runbook_runloop(
    runbook: &mut Runbook,
    progress_tx: &txtx_addon_kit::channel::Sender<BlockEvent>,
) -> Result<(), Vec<Diagnostic>> {
    runbook.supervision_context = RunbookSupervisionContext {
        review_input_default_values: false,
        review_input_values: false,
        is_supervised: false,
    };

    for flow_context in runbook.flow_contexts.iter_mut() {
        if !flow_context.is_enabled() {
            continue;
        }

        let mut action_item_requests = BTreeMap::new();
        let action_item_responses = BTreeMap::new();

        let pass_results = run_signers_evaluation(
            &flow_context.workspace_context,
            &mut flow_context.execution_context,
            &runbook.runtime_context,
            &runbook.supervision_context,
            &mut action_item_requests,
            &action_item_responses,
            &progress_tx,
        )
        .await;

        if pass_results.actions.has_pending_actions() {
            return Err(vec![diagnosed_error!(
                "unsupervised executions should not be generating actions"
            )]);
        }

        if pass_results.has_diagnostics() {
            return Err(pass_results.with_spans_filled(&runbook.sources));
        }

        let mut uuid = Uuid::new_v4();
        let mut background_tasks_futures = vec![];
        let mut background_tasks_contructs_dids = vec![];
        let mut runbook_completed = false;

        loop {
            let mut pass_results = run_constructs_evaluation(
                &uuid,
                &flow_context.workspace_context,
                &mut flow_context.execution_context,
                &mut runbook.runtime_context,
                &runbook.supervision_context,
                &mut action_item_requests,
                &action_item_responses,
                &progress_tx,
            )
            .await;

            if pass_results.has_diagnostics() {
                return Err(pass_results.with_spans_filled(&runbook.sources));
            }

            if !pass_results.pending_background_tasks_constructs_uuids.is_empty() {
                background_tasks_futures.append(&mut pass_results.pending_background_tasks_futures);
                background_tasks_contructs_dids
                    .append(&mut pass_results.pending_background_tasks_constructs_uuids);
            }

            if !pass_results.actions.has_pending_actions()
                && background_tasks_contructs_dids.is_empty()
                && pass_results.nodes_to_re_execute.is_empty()
            {
                runbook_completed = true;
            }

            if background_tasks_futures.is_empty() {
                // sleep(time::Duration::from_secs(3));
            } else {
                process_background_tasks(
                    None,
                    background_tasks_contructs_dids,
                    background_tasks_futures,
                    flow_context,
                )
                .await
                .map_err(|mut diag| {
                    diag.span = get_source_context_for_diagnostic(&diag, &runbook.sources);
                    vec![diag]
                })?;
                background_tasks_futures = vec![];
                background_tasks_contructs_dids = vec![];
            }

            uuid = Uuid::new_v4();
            if runbook_completed {
                break;
            }
        }
    }

    Ok(())
}

/// Returns the index of the next enabled (non-`Ignored`) flow at or after `start`,
/// or `None` if every remaining flow has `RunbookExecutionMode::Ignored`.
///
/// This is the supervised counterpart to the `if !flow_context.is_enabled() { continue; }`
/// skip in [`start_unsupervised_runbook_runloop`]. The state-diff layer marks already-
/// completed flows as [`RunbookExecutionMode::Ignored`] (see
/// `crates/txtx-core/src/runbook/mod.rs` `prepared_flows_for_updated_plans`), and both
/// the initial-flow pick and the post-completion advancement in the supervised runloop
/// must honor that signal so the user is not forced to step through empty genesis panels
/// for flows that ran in a previous invocation.
fn next_enabled_flow_index(
    flow_contexts: &[FlowContext],
    start: usize,
) -> Option<usize> {
    flow_contexts
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, ctx)| ctx.is_enabled())
        .map(|(idx, _)| idx)
}

pub async fn start_supervised_runbook_runloop(
    runbook: &mut Runbook,
    block_tx: Sender<BlockEvent>,
    mut action_item_responses_rx: tokio::sync::broadcast::Receiver<ActionItemResponse>,
) -> Result<(), Vec<Diagnostic>> {
    // let mut runbook_state = BTreeMap::new();

    let mut intialized_flow_index: i16 = -1;
    runbook.supervision_context = RunbookSupervisionContext {
        review_input_default_values: true,
        review_input_values: true,
        is_supervised: true,
    };

    // Compute number of steps
    // A step is

    // store of action_item_ids and the associated action_item_request, grouped by the flow index
    let mut flow_action_item_requests: BTreeMap<usize, BTreeMap<BlockId, ActionItemRequest>> =
        BTreeMap::new();
    // store of construct_dids and its associated action_item_response_types, grouped by the flow index
    let mut flow_action_item_responses = BTreeMap::new();

    let mut background_tasks_futures = vec![];
    let mut background_tasks_contructs_dids = vec![];
    let mut background_tasks_handle_uuid = Uuid::new_v4();
    let mut validated_blocks = 0;
    let total_flows_count = runbook.flow_contexts.len();
    // Honor `RunbookExecutionMode::Ignored` on stateful re-runs: skip over any leading
    // flows that already completed in a previous invocation so the supervisor surfaces
    // the genesis panel for the first flow with actual work to do. If every flow is
    // `Ignored` (e.g. user re-runs an unchanged stateful runbook), there is nothing
    // for the user to do — emit `RunbookCompleted` immediately.
    let mut current_flow_index: usize =
        match next_enabled_flow_index(&runbook.flow_contexts, 0) {
            Some(idx) => idx,
            None => {
                let _ = block_tx.send(BlockEvent::RunbookCompleted(vec![]));
                return Ok(());
            }
        };
    loop {
        let event_opt = match action_item_responses_rx.try_recv() {
            Ok(action) => Some(action),
            Err(TryRecvError::Empty) | Err(TryRecvError::Lagged(_)) => None,
            Err(TryRecvError::Closed) => return Ok(()),
        };

        if intialized_flow_index != current_flow_index as i16 {
            intialized_flow_index = current_flow_index as i16;

            flow_action_item_responses.insert(current_flow_index, BTreeMap::new());
            flow_action_item_requests.insert(current_flow_index, BTreeMap::new());

            let action_item_responses =
                flow_action_item_responses.get_mut(&current_flow_index).unwrap();
            let mut action_item_requests =
                flow_action_item_requests.get_mut(&current_flow_index).unwrap();

            let genesis_events = build_genesis_panel(
                runbook,
                &mut action_item_requests,
                &action_item_responses,
                &block_tx.clone(),
                validated_blocks,
                current_flow_index,
                total_flows_count,
            )
            .await?;
            for event in genesis_events {
                let _ = block_tx.send(event).unwrap();
            }
        }
        let action_item_responses =
            flow_action_item_responses.get_mut(&current_flow_index).unwrap();
        let mut action_item_requests =
            flow_action_item_requests.get_mut(&current_flow_index).unwrap();

        // Cooldown
        let Some(action_item_response) = event_opt else {
            sleep(Duration::from_millis(50));
            continue;
        };
        let ActionItemResponse { action_item_id, payload } = action_item_response.clone();

        if action_item_id == SET_ENV_ACTION.id {
            if let Err(diags) = reset_runbook_execution(
                runbook,
                &payload,
                &mut action_item_requests,
                &action_item_responses,
                &block_tx.clone(),
                current_flow_index,
                total_flows_count,
            )
            .await
            {
                let _ = block_tx.send(BlockEvent::Error(Block {
                    uuid: Uuid::new_v4(),
                    visible: true,
                    panel: Panel::ErrorPanel(ErrorPanelData::from_diagnostics(&diags)),
                }));
                return Err(diags);
            };
            continue;
        }

        if let Some(action_item) = action_item_requests.get(&action_item_id) {
            let action_item = action_item.clone();
            if let Some(construct_did) = action_item.construct_did {
                if let Some(responses) = action_item_responses.get_mut(&construct_did) {
                    responses.push(action_item_response);
                } else {
                    action_item_responses.insert(construct_did, vec![action_item_response]);
                }
            }
        }

        match &payload {
            ActionItemResponseType::ValidateModal => {}
            ActionItemResponseType::ValidateBlock => {
                // Keep track of whether we've initialized this bg uuid to avoid sending more updates
                // for this action item than necessary
                let mut bg_uuid_initialized = false;

                // When a block is validated, the pass could have some set of nested constructs. Each of these constructs
                // needs to have their background tasks awaited before continuing to the next.
                // So in this loop we:
                // 1. Await background tasks, if we have any
                // 2. Evaluate the graph to get new actions
                //   a. If there are no new actions or new pending background tasks, mark the runbook as completed
                //   b. If the runbook isn't completed yet, and there were background tasks at the start of the loop, and we have new background tasks,
                //      we need to loop again to flush out the background tasks
                //   c. If there are new actions and there are no background tasks to await, add the actions to the action item requests and send them to the block processor
                //      to be processed by the frontend
                loop {
                    let start_of_loop_had_bg_tasks = !background_tasks_futures.is_empty();
                    // Handle background tasks
                    if start_of_loop_had_bg_tasks {
                        let flow_context =
                            runbook.flow_contexts.get_mut(current_flow_index).unwrap();
                        let supervised_bg_context = if bg_uuid_initialized {
                            None
                        } else {
                            Some(SupervisedBackgroundTaskContext::new(&block_tx, &action_item_id))
                        };
                        process_background_tasks(
                            supervised_bg_context,
                            background_tasks_contructs_dids,
                            background_tasks_futures,
                            flow_context,
                        )
                        .await
                        .map_err(|mut diag| {
                            diag.span = get_source_context_for_diagnostic(&diag, &runbook.sources);
                            vec![diag]
                        })?;
                        bg_uuid_initialized = true;
                        background_tasks_futures = vec![];
                        background_tasks_contructs_dids = vec![];
                    }

                    // Retrieve the previous requests sent and update their statuses.
                    let mut flow_execution_completed = false;
                    let mut map: BTreeMap<ConstructDid, _> = BTreeMap::new();
                    let flow_context = runbook.flow_contexts.get_mut(current_flow_index).unwrap();
                    let mut pass_results = run_constructs_evaluation(
                        &background_tasks_handle_uuid,
                        &flow_context.workspace_context,
                        &mut flow_context.execution_context,
                        &mut runbook.runtime_context,
                        &runbook.supervision_context,
                        &mut map,
                        &action_item_responses,
                        &block_tx.clone(),
                    )
                    .await;

                    // if there were errors, return them to complete execution
                    if let Some(error_event) = pass_results.compile_diagnostics_to_block() {
                        let _ = block_tx.send(BlockEvent::Error(error_event));
                        return Err(pass_results.with_spans_filled(&runbook.sources));
                    }

                    let pass_has_pending_bg_tasks =
                        !pass_results.pending_background_tasks_constructs_uuids.is_empty();
                    let pass_has_pending_actions = pass_results.actions.has_pending_actions();
                    let pass_has_nodes_to_re_execute = !pass_results.nodes_to_re_execute.is_empty();

                    let additional_info = flow_context
                        .execution_context
                        .commands_execution_results
                        .values()
                        .filter_map(|result| result.runbook_complete_additional_info())
                        .collect::<Vec<_>>();

                    if !pass_has_pending_actions
                        && !pass_has_pending_bg_tasks
                        && !pass_has_nodes_to_re_execute
                    {
                        let flow_context =
                            runbook.flow_contexts.get_mut(current_flow_index).unwrap();
                        let grouped_actions_items =
                            flow_context.execution_context.collect_outputs_constructs_results(
                                &runbook.runtime_context.authorization_context,
                            );
                        let mut actions = Actions::new_panel("output review", "");
                        for (key, action_items) in grouped_actions_items.into_iter() {
                            actions.push_group(key.as_str(), action_items);
                        }
                        pass_results.actions.append(&mut actions);

                        flow_execution_completed = true;
                    } else if !pass_results.actions.store.is_empty() {
                        validated_blocks = validated_blocks + 1;
                        pass_results.actions.push_sub_group(
                            None,
                            vec![ActionItemRequestType::ValidateBlock(ValidateBlockData::new(
                                validated_blocks,
                            ))
                            .to_request("Validate", ACTION_ITEM_VALIDATE_BLOCK)],
                        );
                    }

                    if pass_has_pending_bg_tasks {
                        background_tasks_futures
                            .append(&mut pass_results.pending_background_tasks_futures);
                        background_tasks_contructs_dids
                            .append(&mut pass_results.pending_background_tasks_constructs_uuids);
                    }

                    if !pass_has_pending_bg_tasks && !start_of_loop_had_bg_tasks {
                        let update = ActionItemRequestUpdate::from_id(&action_item_id)
                            .set_status(ActionItemStatus::Success(None));
                        pass_results.actions.push_action_item_update(update);
                        for new_request in
                            pass_results.actions.get_new_action_item_requests().into_iter()
                        {
                            action_item_requests
                                .insert(new_request.id.clone(), new_request.clone());
                        }
                        let block_events = pass_results
                            .actions
                            .compile_actions_to_block_events(&action_item_requests);

                        for event in block_events.into_iter() {
                            let _ = block_tx.send(event);
                        }
                    }
                    if flow_execution_completed && !start_of_loop_had_bg_tasks {
                        // Advance to the next *enabled* flow, skipping any flows that the
                        // state-diff marked as `Ignored`. If none remain, the runbook is done.
                        match next_enabled_flow_index(
                            &runbook.flow_contexts,
                            current_flow_index + 1,
                        ) {
                            Some(next_idx) => {
                                current_flow_index = next_idx;
                            }
                            None => {
                                let _ =
                                    block_tx.send(BlockEvent::RunbookCompleted(additional_info));
                                return Ok(());
                            }
                        }
                    }
                    if !pass_has_pending_bg_tasks
                        && !start_of_loop_had_bg_tasks
                        && !pass_has_nodes_to_re_execute
                    {
                        background_tasks_handle_uuid = Uuid::new_v4();
                        break;
                    }
                }
            }
            ActionItemResponseType::PickInputOption(_) => {}
            ActionItemResponseType::ProvideInput(_) => {}
            ActionItemResponseType::ReviewInput(ReviewedInputResponse {
                value_checked,
                force_execution,
                ..
            }) => {
                let new_status = match value_checked {
                    true => ActionItemStatus::Success(None),
                    false => ActionItemStatus::Todo,
                };
                if let Some(update) = ActionItemRequestUpdate::from_id(&action_item_id)
                    .set_status(new_status)
                    .normalize(&action_item_requests)
                {
                    let _ = block_tx.send(BlockEvent::UpdateActionItems(vec![update]));
                }
                // Some signers do not actually need the user to provide the address/pubkey,
                // but they need to confirm it in the supervisor. when it is confirmed, we need to
                // reprocess the signers
                if let Some(request) = action_item_requests.get(&action_item_id) {
                    if request.internal_key == ACTION_ITEM_CHECK_ADDRESS
                        || request.internal_key == ACTION_ITEM_CHECK_BALANCE
                    {
                        process_signers_action_item_response(
                            runbook,
                            &block_tx,
                            &action_item_id,
                            &mut action_item_requests,
                            &action_item_responses,
                            current_flow_index,
                        )
                        .await;
                    }
                }

                if *force_execution {
                    let running_context =
                        runbook.flow_contexts.get_mut(current_flow_index).unwrap();
                    let mut pass_results = run_constructs_evaluation(
                        &background_tasks_handle_uuid,
                        &running_context.workspace_context,
                        &mut running_context.execution_context,
                        &mut runbook.runtime_context,
                        &runbook.supervision_context,
                        &mut BTreeMap::new(),
                        &action_item_responses,
                        &block_tx.clone(),
                    )
                    .await;
                    let mut updated_actions = vec![];
                    for action in pass_results
                        .actions
                        .compile_actions_to_item_updates(&action_item_requests)
                        .into_iter()
                    {
                        updated_actions.push(action.normalize(&action_item_requests).unwrap())
                    }
                    let _ = block_tx.send(BlockEvent::UpdateActionItems(updated_actions));

                    if !pass_results.pending_background_tasks_constructs_uuids.is_empty() {
                        background_tasks_futures
                            .append(&mut pass_results.pending_background_tasks_futures);
                        background_tasks_contructs_dids
                            .append(&mut pass_results.pending_background_tasks_constructs_uuids);
                    }

                    if pass_results.has_diagnostics() {
                        pass_results.fill_diagnostic_span(&runbook.sources);
                    }
                    if let Some(error_event) = pass_results.compile_diagnostics_to_block() {
                        let _ = block_tx.send(BlockEvent::Error(error_event));
                        return Err(pass_results.with_spans_filled(&runbook.sources));
                    }
                }
            }
            ActionItemResponseType::ProvidePublicKey(_response) => {
                process_signers_action_item_response(
                    runbook,
                    &block_tx,
                    &action_item_id,
                    &mut action_item_requests,
                    &action_item_responses,
                    current_flow_index,
                )
                .await;
            }
            ActionItemResponseType::VerifyThirdPartySignature(_)
            | ActionItemResponseType::ProvideSignedTransaction(_)
            | ActionItemResponseType::SendTransaction(_)
            | ActionItemResponseType::ProvideSignedMessage(_) => {
                // Retrieve the previous requests sent and update their statuses.
                let Some((signing_action_construct_did, scoped_requests)) =
                    retrieve_related_action_items_requests(
                        &action_item_id,
                        &mut action_item_requests,
                    )
                else {
                    continue;
                };
                let mut map: BTreeMap<ConstructDid, _> = BTreeMap::new();
                map.insert(signing_action_construct_did, scoped_requests);

                let running_context = runbook.flow_contexts.get_mut(current_flow_index).unwrap();
                let mut pass_results = run_constructs_evaluation(
                    &background_tasks_handle_uuid,
                    &running_context.workspace_context,
                    &mut running_context.execution_context,
                    &mut runbook.runtime_context,
                    &runbook.supervision_context,
                    &mut map,
                    &action_item_responses,
                    &block_tx.clone(),
                )
                .await;

                let mut updated_actions = vec![];
                for action in pass_results
                    .actions
                    .compile_actions_to_item_updates(&action_item_requests)
                    .into_iter()
                {
                    updated_actions.push(action.normalize(&action_item_requests).unwrap())
                }

                let _ = block_tx.send(BlockEvent::UpdateActionItems(updated_actions));

                if !pass_results.pending_background_tasks_constructs_uuids.is_empty() {
                    background_tasks_futures
                        .append(&mut pass_results.pending_background_tasks_futures);
                    background_tasks_contructs_dids
                        .append(&mut pass_results.pending_background_tasks_constructs_uuids);
                }
                if pass_results.has_diagnostics() {
                    pass_results.fill_diagnostic_span(&runbook.sources);
                }
                if let Some(error_event) = pass_results.compile_diagnostics_to_block() {
                    let _ = block_tx.send(BlockEvent::Error(error_event));
                    return Err(pass_results.with_spans_filled(&runbook.sources));
                }
            }
        };
    }
}

pub fn register_action_items_from_actions(
    actions: &Actions,
    action_item_requests: &mut BTreeMap<BlockId, ActionItemRequest>,
) {
    for action in actions.get_new_action_item_requests().into_iter() {
        action_item_requests.insert(action.id.clone(), action.clone());
    }
}

pub fn retrieve_related_action_items_requests<'a>(
    action_item_id: &BlockId,
    action_item_requests: &'a mut BTreeMap<BlockId, ActionItemRequest>,
) -> Option<(ConstructDid, Vec<&'a mut ActionItemRequest>)> {
    let Some(signer_construct_did) =
        action_item_requests.get(&action_item_id).and_then(|a| a.construct_did.clone())
    else {
        eprintln!("unable to retrieve {}", action_item_id);
        // todo: log error
        return None;
    };
    // // Retrieve the previous requests sent
    // // and update their statuses.
    let mut scoped_requests = vec![];
    for (_, request) in action_item_requests.iter_mut() {
        let Some(ref construct_did) = request.construct_did else {
            continue;
        };
        if construct_did.eq(&signer_construct_did) {
            scoped_requests.push(request);
        }
    }
    Some((signer_construct_did, scoped_requests))
}

pub async fn reset_runbook_execution(
    runbook: &mut Runbook,
    payload: &ActionItemResponseType,
    action_item_requests: &mut BTreeMap<BlockId, ActionItemRequest>,
    action_item_responses: &BTreeMap<ConstructDid, Vec<ActionItemResponse>>,
    progress_tx: &Sender<BlockEvent>,
    current_flow_index: usize,
    total_flows_count: usize,
) -> Result<(), Vec<Diagnostic>> {
    let ActionItemResponseType::PickInputOption(environment_key) = payload else {
        unreachable!(
            "Action item event wih environment uuid sent with invalid payload {:?}",
            payload
        );
    };

    let reset = runbook.update_inputs_selector(Some(environment_key.to_string()), true).await?;

    if !reset {
        unimplemented!()
    }

    let _ = progress_tx.send(BlockEvent::Clear);
    // After an environment switch the per-flow execution modes may have been recomputed;
    // re-seek to the first enabled flow so we never build a genesis panel for a flow
    // that has already completed in the new environment's state. If none remain enabled,
    // signal completion to the caller via `RunbookCompleted`.
    let effective_flow_index =
        next_enabled_flow_index(&runbook.flow_contexts, 0).unwrap_or(current_flow_index);
    let genesis_events = build_genesis_panel(
        runbook,
        action_item_requests,
        action_item_responses,
        &progress_tx,
        0,
        effective_flow_index,
        total_flows_count,
    )
    .await?;
    for event in genesis_events {
        let _ = progress_tx.send(event).unwrap();
    }
    Ok(())
}

pub async fn build_genesis_panel(
    runbook: &mut Runbook,
    action_item_requests: &mut BTreeMap<BlockId, ActionItemRequest>,
    action_item_responses: &BTreeMap<ConstructDid, Vec<ActionItemResponse>>,
    progress_tx: &Sender<BlockEvent>,
    validated_blocks: usize,
    current_flow_index: usize,
    total_flows_count: usize,
) -> Result<Vec<BlockEvent>, Vec<Diagnostic>> {
    let mut actions = Actions::none();

    let environments = runbook.get_inputs_selectors();
    let selector = runbook.get_active_inputs_selector();

    let Some(flow_context) = runbook.flow_contexts.get_mut(current_flow_index) else {
        return Err(vec![diagnosed_error!(
            "internal error: attempted to access a flow that does not exist"
        )]);
    };

    // Defense-in-depth: if a caller passes the index of a flow that was marked
    // `RunbookExecutionMode::Ignored` by the state-diff (i.e. already executed in a
    // previous invocation of this stateful runbook), do not build a signers checklist
    // or "Start Runbook" prompt for it. The supervised runloop normally skips past
    // ignored flows via `next_enabled_flow_index`; this guard ensures the bug cannot
    // resurface from a future caller that forgets to do the skip.
    if !flow_context.is_enabled() {
        return Ok(vec![]);
    }

    if total_flows_count > 1 {
        actions.push_begin_flow_panel(
            current_flow_index,
            total_flows_count,
            &flow_context.name,
            &flow_context.description,
        );
    } else {
    }
    actions.push_panel("runbook checklist", "");

    if environments.len() > 0 {
        let input_options: Vec<InputOption> = environments
            .iter()
            .map(|k| InputOption { value: k.to_string(), displayed_value: k.to_string() })
            .collect();
        let selected_option: InputOption = selector
            .clone()
            .and_then(|e| Some(InputOption { value: e.clone(), displayed_value: e.clone() }))
            .unwrap_or({
                let k = environments.iter().next().unwrap();
                InputOption { value: k.clone(), displayed_value: k.clone() }
            });

        let action_request = ActionItemRequestType::PickInputOption(PickInputOptionRequest {
            options: input_options,
            selected: selected_option,
        })
        .to_request("", ACTION_ITEM_ENV)
        .with_meta_description("Select the environment to target")
        .with_status(ActionItemStatus::Success(None));

        actions.push_sub_group(None, vec![action_request]);
    }

    let mut pass_result: eval::EvaluationPassResult = run_signers_evaluation(
        &flow_context.workspace_context,
        &mut flow_context.execution_context,
        &runbook.runtime_context,
        &runbook.supervision_context,
        &mut BTreeMap::new(),
        &action_item_responses,
        &progress_tx,
    )
    .await;

    if pass_result.has_diagnostics() {
        return Err(pass_result.with_spans_filled(&runbook.sources));
    }

    actions.append(&mut pass_result.actions);

    let validate_action =
        ActionItemRequestType::ValidateBlock(ValidateBlockData::new(validated_blocks))
            .to_request("start runbook", ACTION_ITEM_GENESIS);

    actions.push_sub_group(None, vec![validate_action]);

    register_action_items_from_actions(&actions, action_item_requests);

    let panels = actions.compile_actions_to_block_events(&action_item_requests);
    for panel in panels.iter() {
        match panel {
            BlockEvent::Modal(_) => {}
            BlockEvent::Action(_) => {}
            _ => {
                println!("-----");
            }
        }
    }
    // assert_eq!(panels.len(), 1);

    Ok(panels)
}

#[derive(Debug, Clone)]
pub struct SupervisedBackgroundTaskContext {
    block_tx: Sender<BlockEvent>,
    action_item_id: BlockId,
}
impl SupervisedBackgroundTaskContext {
    pub fn new(block_tx: &Sender<BlockEvent>, action_item_id: &BlockId) -> Self {
        SupervisedBackgroundTaskContext {
            block_tx: block_tx.clone(),
            action_item_id: action_item_id.clone(),
        }
    }
}

pub async fn process_background_tasks(
    supervised_context: Option<SupervisedBackgroundTaskContext>,
    background_tasks_contructs_dids: Vec<(ConstructDid, ConstructDid)>,
    background_tasks_futures: Vec<
        Pin<Box<dyn Future<Output = Result<CommandExecutionResult, Diagnostic>> + Send>>,
    >,
    flow_context: &mut FlowContext,
) -> Result<(), Diagnostic> {
    if let Some(SupervisedBackgroundTaskContext { block_tx, action_item_id, .. }) =
        supervised_context.as_ref()
    {
        let _ =
            block_tx.send(BlockEvent::UpdateActionItems(vec![NormalizedActionItemRequestUpdate {
                id: action_item_id.clone(),
                action_status: Some(ActionItemStatus::Success(None)),
                action_type: None,
            }]));
    }

    let results: Vec<Result<CommandExecutionResult, Diagnostic>> =
        txtx_addon_kit::futures::future::join_all(background_tasks_futures).await;
    for ((nested_construct_did, construct_did), result) in
        background_tasks_contructs_dids.into_iter().zip(results)
    {
        match result {
            Ok(result) => {
                flow_context
                    .execution_context
                    .append_commands_execution_result(&nested_construct_did, &result);
            }
            Err(mut diag) => {
                let construct_id =
                    flow_context.workspace_context.expect_construct_id(&construct_did);
                diag = diag.location(&construct_id.construct_location);
                if let Some(command_instance) =
                    flow_context.execution_context.commands_instances.get_mut(&construct_did)
                {
                    diag = diag.set_span_range(command_instance.block.span());
                };
                if let Some(SupervisedBackgroundTaskContext { block_tx, .. }) =
                    supervised_context.as_ref()
                {
                    let _ = block_tx.send(BlockEvent::Error(Block {
                        uuid: Uuid::new_v4(),
                        visible: true,
                        panel: Panel::ErrorPanel(ErrorPanelData::from_diagnostics(&vec![
                            diag.clone()
                        ])),
                    }));
                }
                return Err(diag);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod skip_ignored_flows_tests {
    use super::*;
    use crate::runbook::RunbookExecutionMode;
    use txtx_addon_kit::types::stores::ValueStore;
    use txtx_addon_kit::types::{Did, RunbookId};

    fn make_flow(name: &str, mode: RunbookExecutionMode) -> FlowContext {
        let runbook_id = RunbookId::zero();
        let inputs = ValueStore::new(name, &Did::zero());
        let mut ctx = FlowContext::new(name, &runbook_id, &inputs);
        ctx.execution_context.execution_mode = mode;
        ctx
    }

    #[test]
    fn returns_first_index_when_all_enabled() {
        let flows = vec![
            make_flow("a", RunbookExecutionMode::Full),
            make_flow("b", RunbookExecutionMode::Full),
        ];
        assert_eq!(next_enabled_flow_index(&flows, 0), Some(0));
        assert_eq!(next_enabled_flow_index(&flows, 1), Some(1));
    }

    #[test]
    fn skips_leading_ignored_flows() {
        let flows = vec![
            make_flow("a", RunbookExecutionMode::Ignored),
            make_flow("b", RunbookExecutionMode::Ignored),
            make_flow("c", RunbookExecutionMode::Full),
        ];
        assert_eq!(next_enabled_flow_index(&flows, 0), Some(2));
    }

    #[test]
    fn returns_none_when_all_ignored() {
        let flows = vec![
            make_flow("a", RunbookExecutionMode::Ignored),
            make_flow("b", RunbookExecutionMode::Ignored),
        ];
        assert_eq!(next_enabled_flow_index(&flows, 0), None);
    }

    #[test]
    fn skips_ignored_flows_between_enabled_ones_when_advancing() {
        let flows = vec![
            make_flow("a", RunbookExecutionMode::Full),
            make_flow("b", RunbookExecutionMode::Ignored),
            make_flow("c", RunbookExecutionMode::Ignored),
            make_flow("d", RunbookExecutionMode::Full),
        ];
        // Caller has just finished flow 0 and is advancing.
        assert_eq!(next_enabled_flow_index(&flows, 1), Some(3));
        // No more flows after flow 3.
        assert_eq!(next_enabled_flow_index(&flows, 4), None);
    }

    #[test]
    fn partial_mode_is_treated_as_enabled() {
        let flows =
            vec![make_flow("a", RunbookExecutionMode::Partial(vec![]))];
        assert_eq!(next_enabled_flow_index(&flows, 0), Some(0));
    }
}

pub async fn process_signers_action_item_response(
    runbook: &mut Runbook,
    block_tx: &Sender<BlockEvent>,
    action_item_id: &BlockId,
    action_item_requests: &mut BTreeMap<BlockId, ActionItemRequest>,
    action_item_responses: &BTreeMap<ConstructDid, Vec<ActionItemResponse>>,
    current_flow_index: usize,
) {
    // Retrieve the previous requests sent and update their statuses.
    let Some((signer_construct_did, scoped_requests)) =
        retrieve_related_action_items_requests(&action_item_id, action_item_requests)
    else {
        return;
    };

    let mut map = BTreeMap::new();
    map.insert(signer_construct_did, scoped_requests);

    let flow_context = runbook.flow_contexts.get_mut(current_flow_index).unwrap();
    let mut pass_result = run_signers_evaluation(
        &flow_context.workspace_context,
        &mut flow_context.execution_context,
        &mut runbook.runtime_context,
        &runbook.supervision_context,
        &mut map,
        &action_item_responses,
        &block_tx.clone(),
    )
    .await;

    if pass_result.has_diagnostics() {
        pass_result.fill_diagnostic_span(&runbook.sources);
    }

    if let Some(error_event) = pass_result.compile_diagnostics_to_block() {
        let _ = block_tx.send(BlockEvent::Error(error_event));
    } else {
        let updated_actions = pass_result
            .actions
            .compile_actions_to_item_updates(&action_item_requests)
            .into_iter()
            .map(|u| u.normalize(&action_item_requests).unwrap())
            .collect();
        let _ = block_tx.send(BlockEvent::UpdateActionItems(updated_actions));
    }
}
