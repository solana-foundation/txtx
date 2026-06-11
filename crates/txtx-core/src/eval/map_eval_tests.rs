use super::*;
use kit::hcl::parser::parse_body;
use kit::hcl::structure::Block as HclBlock;
use kit::types::commands::{CommandInputsEvaluationResult, DependencyExecutionResultCache};
use kit::types::diagnostics::Diagnostic;
use kit::types::stores::ValueMap;
use kit::types::types::{ObjectDefinition, ObjectProperty, Type, Value};
use kit::types::{AuthorizationContext, EvaluatableInput, PackageId, RunbookId, WithEvaluatableInputs};
use kit::Addon;

use crate::runbook::{RunbookWorkspaceContext, RuntimeContext};
use crate::std::StdAddon;
use crate::types::RunbookExecutionContext;
use txtx_addon_kit::types::cloud_interface::CloudServiceContext;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn get_addon_by_namespace(namespace: &str) -> Option<Box<dyn Addon>> {
    let available_addons: Vec<Box<dyn Addon>> = vec![Box::new(StdAddon::new())];
    for addon in available_addons.into_iter() {
        if namespace.starts_with(&format!("{}", addon.get_namespace())) {
            return Some(addon);
        }
    }
    None
}

/// Build all the context objects needed by the evaluation functions.
fn make_contexts() -> (
    DependencyExecutionResultCache,
    PackageId,
    RunbookWorkspaceContext,
    RunbookExecutionContext,
    RuntimeContext,
) {
    let deps = DependencyExecutionResultCache::new();
    let package_id = PackageId::zero();
    let runbook_id = RunbookId::zero();
    let workspace_ctx = RunbookWorkspaceContext::new(runbook_id);
    let execution_ctx = RunbookExecutionContext::new();
    let runtime_ctx = RuntimeContext::new(
        AuthorizationContext::empty(),
        get_addon_by_namespace,
        CloudServiceContext::empty(),
    );
    (deps, package_id, workspace_ctx, execution_ctx, runtime_ctx)
}

/// Parse an HCL string and return the contained blocks.
fn parse_blocks(hcl: &str) -> Vec<HclBlock> {
    let body = parse_body(hcl).expect("failed to parse HCL");
    body.into_blocks().collect()
}

// ---------------------------------------------------------------------------
// evaluate_arbitrary_inputs_map tests
// ---------------------------------------------------------------------------

#[test]
fn arbitrary_map_single_block_with_literal_attributes() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    name = "hello"
    count = 42
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(!result.fatal_error);
    assert!(!result.require_user_interaction);
    assert!(result.diags.is_empty());
    assert_eq!(result.entries.len(), 1, "should produce one entry per block");

    let obj = result.entries[0].as_object().expect("entry should be an object");
    assert_eq!(obj.get("name").unwrap(), &Value::string("hello".to_string()));
    assert_eq!(obj.get("count").unwrap(), &Value::integer(42));
}

#[test]
fn arbitrary_map_multiple_blocks_produce_separate_entries() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
item {
    a = 1
}
item {
    a = 2
}
item {
    a = 3
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "item",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 3, "should produce one entry per block");

    for (i, entry) in result.entries.iter().enumerate() {
        let obj = entry.as_object().expect("entry should be an object");
        let expected = (i + 1) as i128;
        assert_eq!(
            obj.get("a").unwrap(),
            &Value::integer(expected),
            "block {} should have a = {}",
            i,
            expected
        );
    }
}

#[test]
fn arbitrary_map_nested_child_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    program_id = "abc123"
    account {
        address = "wallet1"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(!result.fatal_error);
    assert_eq!(result.entries.len(), 1);

    let obj = result.entries[0].as_object().expect("entry should be an object");
    assert_eq!(
        obj.get("program_id").unwrap(),
        &Value::string("abc123".to_string())
    );

    // The child block "account" should be an array of its entries
    let account_val = obj.get("account").expect("should have 'account' key");
    let account_arr = account_val.as_array().expect("account should be an array");
    assert_eq!(account_arr.len(), 1);

    let account_obj = account_arr[0].as_object().expect("account entry should be an object");
    assert_eq!(
        account_obj.get("address").unwrap(),
        &Value::string("wallet1".to_string())
    );
}

#[test]
fn arbitrary_map_multiple_child_blocks_same_ident() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    account {
        address = "wallet1"
    }
    account {
        address = "wallet2"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();

    let account_arr = obj
        .get("account")
        .expect("should have 'account' key")
        .as_array()
        .expect("account should be an array");
    assert_eq!(account_arr.len(), 2, "should have two account entries");

    let addr0 = account_arr[0].as_object().unwrap().get("address").unwrap().clone();
    let addr1 = account_arr[1].as_object().unwrap().get("address").unwrap().clone();
    assert_eq!(addr0, Value::string("wallet1".to_string()));
    assert_eq!(addr1, Value::string("wallet2".to_string()));
}

/// This is the core regression test for the duplicate-instructions bug.
/// When the same instruction block appears twice, each should produce an
/// independent entry with only its own child block data—not carry over
/// entries from previous blocks.
#[test]
fn arbitrary_map_duplicate_instructions_do_not_leak_entries() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    program_id = "prog1"
    payer {
        address = "payer1"
    }
}
instruction {
    program_id = "prog2"
    payer {
        address = "payer2"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(!result.fatal_error);
    assert_eq!(result.entries.len(), 2, "should produce two entries");

    // First instruction
    let obj0 = result.entries[0].as_object().unwrap();
    assert_eq!(obj0.get("program_id").unwrap(), &Value::string("prog1".to_string()));
    let payer0 = obj0.get("payer").unwrap().as_array().unwrap();
    assert_eq!(payer0.len(), 1, "first instruction's payer should have exactly 1 entry");
    assert_eq!(
        payer0[0].as_object().unwrap().get("address").unwrap(),
        &Value::string("payer1".to_string())
    );

    // Second instruction — this is the one that used to fail because payer had 2 entries
    let obj1 = result.entries[1].as_object().unwrap();
    assert_eq!(obj1.get("program_id").unwrap(), &Value::string("prog2".to_string()));
    let payer1 = obj1.get("payer").unwrap().as_array().unwrap();
    assert_eq!(
        payer1.len(),
        1,
        "second instruction's payer should have exactly 1 entry (regression: was 2 before fix)"
    );
    assert_eq!(
        payer1[0].as_object().unwrap().get("address").unwrap(),
        &Value::string("payer2".to_string())
    );
}

#[test]
fn arbitrary_map_empty_blocks_returns_empty_entries() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        vec![],
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(result.entries.is_empty());
    assert!(!result.fatal_error);
    assert!(!result.require_user_interaction);
}

#[test]
fn arbitrary_map_block_with_no_attributes_or_children() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();
    assert!(obj.is_empty(), "empty block should produce empty object");
}

#[test]
fn arbitrary_map_boolean_and_null_attributes() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
config {
    enabled = true
    disabled = false
    nothing = null
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "config",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();
    assert_eq!(obj.get("enabled").unwrap(), &Value::bool(true));
    assert_eq!(obj.get("disabled").unwrap(), &Value::bool(false));
    assert_eq!(obj.get("nothing").unwrap(), &Value::null());
}

#[test]
fn arbitrary_map_deeply_nested_child_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
root {
    level1 {
        level2 {
            value = "deep"
        }
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "root",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let root_obj = result.entries[0].as_object().unwrap();

    let level1_arr = root_obj.get("level1").unwrap().as_array().unwrap();
    assert_eq!(level1_arr.len(), 1);

    let level1_obj = level1_arr[0].as_object().unwrap();
    let level2_arr = level1_obj.get("level2").unwrap().as_array().unwrap();
    assert_eq!(level2_arr.len(), 1);

    let level2_obj = level2_arr[0].as_object().unwrap();
    assert_eq!(level2_obj.get("value").unwrap(), &Value::string("deep".to_string()));
}

#[test]
fn arbitrary_map_mixed_attributes_and_child_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
tx {
    memo = "transfer"
    amount = 100
    recipient {
        address = "addr1"
    }
    signer {
        key = "key1"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "tx",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();

    // Attributes
    assert_eq!(obj.get("memo").unwrap(), &Value::string("transfer".to_string()));
    assert_eq!(obj.get("amount").unwrap(), &Value::integer(100));

    // Child blocks
    let recipient = obj.get("recipient").unwrap().as_array().unwrap();
    assert_eq!(recipient.len(), 1);
    assert_eq!(
        recipient[0].as_object().unwrap().get("address").unwrap(),
        &Value::string("addr1".to_string())
    );

    let signer = obj.get("signer").unwrap().as_array().unwrap();
    assert_eq!(signer.len(), 1);
    assert_eq!(
        signer[0].as_object().unwrap().get("key").unwrap(),
        &Value::string("key1".to_string())
    );
}

#[test]
fn arbitrary_map_three_duplicate_instructions_isolated_children() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    id = "ix1"
    account {
        name = "acct_a"
    }
}
instruction {
    id = "ix2"
    account {
        name = "acct_b"
    }
}
instruction {
    id = "ix3"
    account {
        name = "acct_c"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 3);

    for (i, (expected_id, expected_acct)) in
        [("ix1", "acct_a"), ("ix2", "acct_b"), ("ix3", "acct_c")]
            .iter()
            .enumerate()
    {
        let obj = result.entries[i].as_object().unwrap();
        assert_eq!(
            obj.get("id").unwrap(),
            &Value::string(expected_id.to_string()),
            "instruction {} id mismatch",
            i
        );
        let accounts = obj.get("account").unwrap().as_array().unwrap();
        assert_eq!(
            accounts.len(),
            1,
            "instruction {} should have exactly 1 account entry",
            i
        );
        assert_eq!(
            accounts[0].as_object().unwrap().get("name").unwrap(),
            &Value::string(expected_acct.to_string()),
        );
    }
}

#[test]
fn arbitrary_map_child_blocks_different_idents_in_same_block() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let blocks = parse_blocks(
        r#"
instruction {
    payer {
        address = "p1"
    }
    authority {
        address = "a1"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();

    let payer = obj.get("payer").unwrap().as_array().unwrap();
    assert_eq!(payer.len(), 1);
    assert_eq!(
        payer[0].as_object().unwrap().get("address").unwrap(),
        &Value::string("p1".to_string())
    );

    let authority = obj.get("authority").unwrap().as_array().unwrap();
    assert_eq!(authority.len(), 1);
    assert_eq!(
        authority[0].as_object().unwrap().get("address").unwrap(),
        &Value::string("a1".to_string())
    );
}

/// Regression test for the `program_idl`-dropped bug. When an instruction
/// attribute (e.g. `program_idl = variable.<...>.idl`) can't resolve on the
/// current pass, the input is marked in `unevaluated_inputs` so it gets
/// re-evaluated later. A sibling account block whose values resolve cleanly
/// must NOT wipe that marker — otherwise the input is never re-evaluated and
/// `program_idl` stays permanently missing.
///
/// We simulate the prior-pass marker by seeding `unevaluated_inputs`, then
/// evaluate a block whose only child block resolves cleanly, and assert the
/// marker survives. Before the fix at eval/mod.rs:2434 (assign instead of
/// merge) the clean child overwrote it to empty and this assertion failed.
#[test]
fn arbitrary_map_clean_child_block_preserves_unevaluated_marker() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let mut parent_result = EvaluateMapObjectPropResult::new();
    parent_result.unevaluated_inputs.insert("instruction".to_string(), None);

    let blocks = parse_blocks(
        r#"
instruction {
    extra_account {
        public_key = "literal_pubkey"
    }
}
"#,
    );

    let result = evaluate_arbitrary_inputs_map(
        "instruction",
        parent_result,
        blocks,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(
        result.unevaluated_inputs.contains_key("instruction"),
        "a cleanly-resolving child block must not clear a pre-existing unevaluated marker \
         (regression: overwrite at eval/mod.rs:2434)"
    );
}

// ---------------------------------------------------------------------------
// evaluate_map_object_prop tests
// ---------------------------------------------------------------------------

fn make_object_prop(name: &str, typing: Type) -> ObjectProperty {
    ObjectProperty {
        name: name.to_string(),
        documentation: String::new(),
        typing,
        optional: false,
        tainting: false,
        internal: false,
    }
}

#[test]
fn strict_map_single_block_with_matching_props() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![
        make_object_prop("name", Type::string()),
        make_object_prop("value", Type::integer()),
    ];

    let blocks = parse_blocks(
        r#"
entry {
    name = "test"
    value = 99
}
"#,
    );

    let result = evaluate_map_object_prop(
        "entry",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(!result.fatal_error);
    assert_eq!(result.entries.len(), 1);

    let obj = result.entries[0].as_object().unwrap();
    assert_eq!(obj.get("name").unwrap(), &Value::string("test".to_string()));
    assert_eq!(obj.get("value").unwrap(), &Value::integer(99));
}

#[test]
fn strict_map_multiple_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![make_object_prop("x", Type::integer())];

    let blocks = parse_blocks(
        r#"
point {
    x = 10
}
point {
    x = 20
}
"#,
    );

    let result = evaluate_map_object_prop(
        "point",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 2);
    assert_eq!(
        result.entries[0].as_object().unwrap().get("x").unwrap(),
        &Value::integer(10)
    );
    assert_eq!(
        result.entries[1].as_object().unwrap().get("x").unwrap(),
        &Value::integer(20)
    );
}

#[test]
fn strict_map_missing_optional_prop_skipped() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![
        make_object_prop("required_field", Type::string()),
        {
            let mut p = make_object_prop("optional_field", Type::string());
            p.optional = true;
            p
        },
    ];

    let blocks = parse_blocks(
        r#"
entry {
    required_field = "present"
}
"#,
    );

    let result = evaluate_map_object_prop(
        "entry",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();
    assert_eq!(
        obj.get("required_field").unwrap(),
        &Value::string("present".to_string())
    );
    // optional_field should not be present since it wasn't in the block
    assert!(obj.get("optional_field").is_none());
}

#[test]
fn strict_map_empty_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![make_object_prop("x", Type::integer())];

    let result = evaluate_map_object_prop(
        "entry",
        EvaluateMapObjectPropResult::new(),
        vec![],
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(result.entries.is_empty());
}

#[test]
fn strict_map_nested_child_map_blocks_arbitrary() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![
        make_object_prop("label", Type::string()),
        make_object_prop("metadata", Type::Map(ObjectDefinition::Arbitrary(None))),
    ];

    let blocks = parse_blocks(
        r#"
entry {
    label = "my_entry"
    metadata {
        key1 = "val1"
        key2 = "val2"
    }
}
"#,
    );

    let result = evaluate_map_object_prop(
        "entry",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();
    assert_eq!(obj.get("label").unwrap(), &Value::string("my_entry".to_string()));

    // metadata should be an array wrapping the arbitrary map result
    let metadata = obj.get("metadata").unwrap().as_array().unwrap();
    assert_eq!(metadata.len(), 1);
    let meta_obj = metadata[0].as_object().unwrap();
    assert_eq!(meta_obj.get("key1").unwrap(), &Value::string("val1".to_string()));
    assert_eq!(meta_obj.get("key2").unwrap(), &Value::string("val2".to_string()));
}

#[test]
fn strict_map_nested_strict_child_map() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let inner_props = vec![make_object_prop("addr", Type::string())];

    let props = vec![
        make_object_prop("name", Type::string()),
        make_object_prop(
            "account",
            Type::Map(ObjectDefinition::Strict(inner_props)),
        ),
    ];

    let blocks = parse_blocks(
        r#"
entry {
    name = "test"
    account {
        addr = "0xabc"
    }
}
"#,
    );

    let result = evaluate_map_object_prop(
        "entry",
        EvaluateMapObjectPropResult::new(),
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert_eq!(result.entries.len(), 1);
    let obj = result.entries[0].as_object().unwrap();
    assert_eq!(obj.get("name").unwrap(), &Value::string("test".to_string()));

    let account_arr = obj.get("account").unwrap().as_array().unwrap();
    assert_eq!(account_arr.len(), 1);
    assert_eq!(
        account_arr[0].as_object().unwrap().get("addr").unwrap(),
        &Value::string("0xabc".to_string())
    );
}

/// Strict-map counterpart of `arbitrary_map_clean_child_block_preserves_unevaluated_marker`.
/// Guards the identical overwrite-vs-merge bug at eval/mod.rs:2573, hit when a
/// strict map prop is itself a nested map whose child block resolves cleanly.
#[test]
fn strict_map_clean_child_map_preserves_unevaluated_marker() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![make_object_prop(
        "metadata",
        Type::Map(ObjectDefinition::Arbitrary(None)),
    )];

    let mut parent_result = EvaluateMapObjectPropResult::new();
    parent_result.unevaluated_inputs.insert("entry".to_string(), None);

    let blocks = parse_blocks(
        r#"
entry {
    metadata {
        key = "v"
    }
}
"#,
    );

    let result = evaluate_map_object_prop(
        "entry",
        parent_result,
        blocks,
        &props,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("evaluation should succeed");

    assert!(
        result.unevaluated_inputs.contains_key("entry"),
        "a cleanly-resolving child map must not clear a pre-existing unevaluated marker \
         (regression: overwrite at eval/mod.rs:2573)"
    );
}

// ---------------------------------------------------------------------------
// evaluate_map_input tests
// ---------------------------------------------------------------------------

/// Minimal mock implementing EvaluatableInput for test purposes.
#[derive(Clone, Debug)]
struct MockEvaluatableInput {
    name: String,
    typing: Type,
    optional: bool,
}

impl EvaluatableInput for MockEvaluatableInput {
    fn documentation(&self) -> String {
        String::new()
    }
    fn optional(&self) -> bool {
        self.optional
    }
    fn typing(&self) -> &Type {
        &self.typing
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

/// Minimal mock implementing WithEvaluatableInputs.
/// Stores HCL blocks to return from `get_blocks_for_map`.
#[derive(Clone, Debug)]
struct MockWithEvaluatableInputs {
    block: HclBlock,
    map_blocks: Option<Vec<HclBlock>>,
}

impl WithEvaluatableInputs for MockWithEvaluatableInputs {
    fn name(&self) -> String {
        "mock".to_string()
    }

    fn block(&self) -> &HclBlock {
        &self.block
    }

    fn get_expression_from_input(
        &self,
        _input_name: &str,
    ) -> Option<kit::hcl::expr::Expression> {
        None
    }

    fn get_blocks_for_map(
        &self,
        _input_name: &str,
        _input_typing: &Type,
        _input_optional: bool,
    ) -> Result<Option<Vec<HclBlock>>, Vec<Diagnostic>> {
        Ok(self.map_blocks.clone())
    }

    fn get_expression_from_block(
        &self,
        _block: &HclBlock,
        _prop: &ObjectProperty,
    ) -> Option<kit::hcl::expr::Expression> {
        None
    }

    fn get_expression_from_object(
        &self,
        _input_name: &str,
        _input_typing: &Type,
    ) -> Result<Option<kit::hcl::expr::Expression>, Vec<Diagnostic>> {
        Ok(None)
    }

    fn get_expression_from_object_property(
        &self,
        _input_name: &str,
        _prop: &ObjectProperty,
    ) -> Option<kit::hcl::expr::Expression> {
        None
    }

    fn _spec_inputs(&self) -> Vec<Box<dyn EvaluatableInput>> {
        vec![]
    }
}

fn make_empty_hcl_block() -> HclBlock {
    let body = parse_body("empty {}").unwrap();
    body.into_blocks().next().unwrap()
}

#[test]
fn evaluate_map_input_returns_none_when_no_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let input_spec: Box<dyn EvaluatableInput> = Box::new(MockEvaluatableInput {
        name: "my_map".to_string(),
        typing: Type::Map(ObjectDefinition::Arbitrary(None)),
        optional: true,
    });

    let mock_inputs = MockWithEvaluatableInputs {
        block: make_empty_hcl_block(),
        map_blocks: None, // no blocks found
    };

    let result = evaluate_map_input(
        CommandInputsEvaluationResult::new("test", &ValueMap::new()),
        &input_spec,
        &mock_inputs,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("should not error");

    assert!(result.is_none(), "should return None when no blocks are found");
}

#[test]
fn evaluate_map_input_arbitrary_with_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let input_spec: Box<dyn EvaluatableInput> = Box::new(MockEvaluatableInput {
        name: "config".to_string(),
        typing: Type::Map(ObjectDefinition::Arbitrary(None)),
        optional: false,
    });

    let blocks = parse_blocks(
        r#"
config {
    key = "value"
    num = 7
}
"#,
    );

    let mock_inputs = MockWithEvaluatableInputs {
        block: make_empty_hcl_block(),
        map_blocks: Some(blocks),
    };

    let result = evaluate_map_input(
        CommandInputsEvaluationResult::new("test", &ValueMap::new()),
        &input_spec,
        &mock_inputs,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("should not error");

    let map_result = result.expect("should return Some");
    assert!(!map_result.fatal_error);
    assert!(!map_result.require_user_interaction);

    // The result should have our "config" key in inputs
    let config_value = map_result.result.inputs.get_value("config");
    assert!(config_value.is_some(), "result should contain 'config' input");

    let arr = config_value.unwrap().as_array().expect("config should be an array");
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().unwrap();
    assert_eq!(obj.get("key").unwrap(), &Value::string("value".to_string()));
    assert_eq!(obj.get("num").unwrap(), &Value::integer(7));
}

#[test]
fn evaluate_map_input_strict_with_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let props = vec![
        make_object_prop("host", Type::string()),
        make_object_prop("port", Type::integer()),
    ];

    let input_spec: Box<dyn EvaluatableInput> = Box::new(MockEvaluatableInput {
        name: "server".to_string(),
        typing: Type::Map(ObjectDefinition::Strict(props)),
        optional: false,
    });

    let blocks = parse_blocks(
        r#"
server {
    host = "localhost"
    port = 8080
}
"#,
    );

    let mock_inputs = MockWithEvaluatableInputs {
        block: make_empty_hcl_block(),
        map_blocks: Some(blocks),
    };

    let result = evaluate_map_input(
        CommandInputsEvaluationResult::new("test", &ValueMap::new()),
        &input_spec,
        &mock_inputs,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("should not error");

    let map_result = result.expect("should return Some");
    assert!(!map_result.fatal_error);

    let server_value = map_result.result.inputs.get_value("server");
    assert!(server_value.is_some());

    let arr = server_value.unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().unwrap();
    assert_eq!(obj.get("host").unwrap(), &Value::string("localhost".to_string()));
    assert_eq!(obj.get("port").unwrap(), &Value::integer(8080));
}

#[test]
fn evaluate_map_input_multiple_arbitrary_blocks() {
    let (deps, pkg, ws, exec, rt) = make_contexts();

    let input_spec: Box<dyn EvaluatableInput> = Box::new(MockEvaluatableInput {
        name: "items".to_string(),
        typing: Type::Map(ObjectDefinition::Arbitrary(None)),
        optional: false,
    });

    let blocks = parse_blocks(
        r#"
items {
    name = "first"
}
items {
    name = "second"
}
"#,
    );

    let mock_inputs = MockWithEvaluatableInputs {
        block: make_empty_hcl_block(),
        map_blocks: Some(blocks),
    };

    let result = evaluate_map_input(
        CommandInputsEvaluationResult::new("test", &ValueMap::new()),
        &input_spec,
        &mock_inputs,
        &deps,
        &pkg,
        &ws,
        &exec,
        &rt,
    )
    .expect("should not error");

    let map_result = result.expect("should return Some");
    let arr = map_result
        .result
        .inputs
        .get_value("items")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(
        arr[0].as_object().unwrap().get("name").unwrap(),
        &Value::string("first".to_string())
    );
    assert_eq!(
        arr[1].as_object().unwrap().get("name").unwrap(),
        &Value::string("second".to_string())
    );
}
