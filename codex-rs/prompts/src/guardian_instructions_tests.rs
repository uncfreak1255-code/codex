//! Covers policy substitution, terminal output contracts, truncation, and rejection-text overrides.

use codex_context_fragments::ContextualUserFragment;
use codex_guardian_context::TRANSCRIPT_JSON_INSTRUCTIONS;
use codex_guardian_context::truncate_text;
use codex_protocol::TranscriptFormat;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol_test_support::ContentItemTestExt;
use pretty_assertions::assert_eq;

use super::GuardianClassifierInstructions;
use super::GuardianPolicyInstructions;
use super::render_guardian_rejection;
use crate::ResolvedModelMessages;

#[test]
fn reviewer_policy_substitution_preserves_template_layout_and_appends_contract() {
    for (template, policy, expected) in [
        (
            " Review:\n{{ tenant_policy_config }}\nAgain: {{ tenant_policy_config }}\n \t",
            " \nTenant policy.\n ",
            " Review:\nTenant policy.\nAgain: Tenant policy.\n\nReview contract.\n",
        ),
        (
            "Policy: {{ tenant_policy_config }}",
            "",
            "Policy: \n\nReview contract.\n",
        ),
        (
            "Tenant: {{ tenant_policy_config }}\nAdditional: {{ extra_policy }}",
            "Tenant policy.",
            "Tenant: Tenant policy.\nAdditional: \n\nReview contract.\n",
        ),
        ("", "Tenant policy.", "\n\nReview contract.\n"),
    ] {
        assert_eq!(
            GuardianPolicyInstructions::new(
                TranscriptFormat::Json,
                policy,
                "",
                template,
                "Review contract."
            )
            .render()
            .expect_text(),
            format!("{TRANSCRIPT_JSON_INSTRUCTIONS}\n\n{expected}"),
        );
    }
}

#[test]
fn reviewer_extra_policy_substitution_preserves_tenant_policy() {
    for (template, extra_policy, expected) in [
        (
            "Tenant: {{ tenant_policy_config }}\nAdditional: {{ extra_policy }}\nAgain: {{ extra_policy }}",
            " \nAdditional policy.\n ",
            "Tenant: Tenant policy.\nAdditional: Additional policy.\nAgain: Additional policy.\n\nReview contract.\n",
        ),
        (
            "Tenant: {{ tenant_policy_config }}\nAdditional: {{ extra_policy }}",
            " \n\t",
            "Tenant: Tenant policy.\nAdditional: \n\nReview contract.\n",
        ),
        (
            "Tenant: {{ tenant_policy_config }}",
            " \nAdditional policy.\n ",
            "Tenant: Tenant policy.\n\n# Additional Policy\nAdditional policy.\n\nReview contract.\n",
        ),
        (
            "Tenant: {{ tenant_policy_config }}",
            " \n\t",
            "Tenant: Tenant policy.\n\nReview contract.\n",
        ),
    ] {
        for transcript_format in [TranscriptFormat::Line, TranscriptFormat::Json] {
            let expected = match transcript_format {
                TranscriptFormat::Line => expected.to_owned(),
                TranscriptFormat::Json => format!("{TRANSCRIPT_JSON_INSTRUCTIONS}\n\n{expected}"),
            };
            assert_eq!(
                GuardianPolicyInstructions::new(
                    transcript_format,
                    "Tenant policy.",
                    extra_policy,
                    template,
                    "Review contract.",
                )
                .render()
                .expect_text(),
                expected,
            );
        }
    }
}

#[test]
fn reviewer_extra_policy_reaches_catalog_selected_reviewers() {
    let catalog: ModelsResponse =
        serde_json::from_str(include_str!("../../models-manager/models.json"))
            .expect("bundled model catalog");
    for slug in ["codex-auto-review", "gpt-5.6-luna"] {
        let mut model = catalog
            .models
            .iter()
            .find(|model| model.slug == slug)
            .expect("bundled reviewer model")
            .clone();
        // A later catalog template must remain effective instead of being replaced
        // by the bundled fallback just to restore the additional-policy slot.
        let template = model
            .model_messages
            .as_mut()
            .expect("catalog reviewer messages")
            .auto_review
            .as_mut()
            .expect("catalog auto-review messages")
            .policy_template
            .as_mut()
            .expect("catalog reviewer template");
        template.insert_str(0, "Catalog-specific reviewer guidance.\n");
        let messages = ResolvedModelMessages::from_model(&model).auto_review();
        for extra_policy in [
            "Require explicit confirmation before deleting protected data.",
            "Standing permission covers read-only inspection of the workspace.",
        ] {
            let rendered = GuardianPolicyInstructions::new(
                TranscriptFormat::Json,
                messages.policy,
                extra_policy,
                messages.policy_template,
                "Review contract.",
            )
            .render()
            .expect_text();
            assert!(rendered.contains("Catalog-specific reviewer guidance."));
            assert!(
                rendered.contains(messages.policy.trim()),
                "{slug}: tenant policy missing"
            );
            assert_eq!(
                rendered.matches(extra_policy).count(),
                1,
                "{slug}: additional policy missing or duplicated"
            );
            assert!(rendered.ends_with("\n\nReview contract.\n"));
        }
    }
}

#[test]
fn reviewer_policy_substitution_keeps_inserted_policy_text_literal() {
    assert_eq!(
        GuardianPolicyInstructions::new(
            TranscriptFormat::Json,
            "Tenant says {{ tenant_policy_config }} and {{ extra_policy }}.",
            "Additional says {{ tenant_policy_config }} and {{ extra_policy }}.",
            "Tenant: {{ tenant_policy_config }}\nAdditional: {{ extra_policy }}",
            "Review contract.",
        )
        .render()
        .expect_text(),
        format!(
            "{TRANSCRIPT_JSON_INSTRUCTIONS}\n\nTenant: Tenant says {{{{ tenant_policy_config }}}} and {{{{ extra_policy }}}}.\nAdditional: Additional says {{{{ tenant_policy_config }}}} and {{{{ extra_policy }}}}.\n\nReview contract.\n"
        ),
    );
    assert_eq!(
        GuardianPolicyInstructions::new(
            TranscriptFormat::Line,
            "Tenant says {{ tenant_policy_config }} and {{ extra_policy }}.",
            "Additional says {{ tenant_policy_config }} and {{ extra_policy }}.",
            "Future catalog instruction.\nTenant: {{ tenant_policy_config }}",
            "Review contract.",
        )
        .render()
        .expect_text(),
        "Future catalog instruction.\nTenant: Tenant says {{ tenant_policy_config }} and {{ extra_policy }}.\n\n# Additional Policy\nAdditional says {{ tenant_policy_config }} and {{ extra_policy }}.\n\nReview contract.\n",
    );
}

#[test]
fn classifier_policy_substitution_and_legacy_append_keep_one_terminal_contract_before_truncation() {
    for (instructions, policy, expected) in [
        (
            "Classify: {{ tenant_policy_config }}",
            " Tenant policy.\n",
            "Classify:  Tenant policy.\n\n\nClassifier contract.",
        ),
        (
            "Classify: {{ tenant_policy_config }}\n\nClassifier contract.\n \t",
            "Tenant policy.",
            "Classify: Tenant policy.\n\nClassifier contract.\n \t",
        ),
        (
            "Legacy classifier.",
            "Tenant policy.",
            "Legacy classifier.\n\n# Security Policy\nTenant policy.\n\nClassifier contract.",
        ),
    ] {
        for max_tokens in [None, Some(20)] {
            let expected = match max_tokens {
                Some(max_tokens) => truncate_text(expected, max_tokens),
                None => expected.to_owned(),
            };
            assert_eq!(
                GuardianClassifierInstructions::new(
                    TranscriptFormat::Json,
                    instructions,
                    policy,
                    "Classifier contract.",
                    max_tokens,
                )
                .render()
                .expect_text(),
                format!("{TRANSCRIPT_JSON_INSTRUCTIONS}\n\n{expected}"),
            );
        }
    }
}

#[test]
fn rejection_feedback_trims_rationale_and_preserves_instruction_overrides() {
    for (rationale, instructions, expected) in [
        (
            " \nSensitive data would leave the workspace.\t ",
            " Ask for approval.\n",
            "This action was rejected due to unacceptable risk.\nReason: Sensitive data would leave the workspace.\n Ask for approval.\n",
        ),
        (
            " \n\t",
            "",
            "This action was rejected due to unacceptable risk.\nReason: Auto-reviewer denied the action without a specific rationale.\n",
        ),
    ] {
        assert_eq!(render_guardian_rejection(rationale, instructions), expected);
    }
}
