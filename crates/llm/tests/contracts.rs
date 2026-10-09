use llm::*;
use std::collections::BTreeMap;

#[test]
fn schema_is_strict_and_prompt_contains_controlled_wording_contract() {
    let input = NarrationInput {
        facts: BTreeMap::from([("quality".into(), "质量：90%".into())]),
        interpretation: "只描述".into(),
        limitations: vec!["局限".into()],
    };
    let prompt: serde_json::Value = serde_json::from_str(&user_prompt(&input).unwrap()).unwrap();
    assert_eq!(prompt["data"]["facts"]["quality"], "质量：90%");
    assert_eq!(
        prompt["allowed_connectives"],
        serde_json::json!(CONNECTIVES)
    );
    assert!(SYSTEM_PROMPT.contains("{{fact_id}}"));
    let schema = narrative_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"].as_array().unwrap().len(), 4);
    assert_eq!(
        schema["properties"]["summary"]["additionalProperties"],
        false
    );
}

#[test]
fn oversize_input_and_unconfigured_provider_fail_explicitly() {
    let input = NarrationInput {
        facts: BTreeMap::new(),
        interpretation: "x".repeat(128 * 1024),
        limitations: vec![],
    };
    assert_eq!(user_prompt(&input), Err(LlmError::InputTooLarge));
    assert_eq!(
        UnconfiguredNarrator.narrate(&input),
        Err(LlmError::NotConfigured)
    );
}
