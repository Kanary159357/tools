use rome_analyze::{context::RuleContext, declare_rule, ActionCategory, RefactorKind, Rule};
use rome_console::markup;
use rome_diagnostics::Applicability;
use rome_js_semantic::{AllReferencesExtensions, Reference};
use rome_js_syntax::{
    JsAnyBinding, JsAnyBindingPattern, JsAnyExpression, JsIdentifierExpression,
    JsVariableDeclarator,
};
use rome_rowan::{BatchMutationExt, SyntaxNodeCast};

use crate::{semantic_services::Semantic, utils::remove_declarator, JsRuleAction};

declare_rule! {
    /// Provides a refactor to inline variables
    ///
    /// ## Examples
    ///
    /// ```js
    /// let variable = expression();
    /// statement(variable);
    /// ```
    pub(crate) InlineVariable {
        version: "0.9.0",
        name: "inlineVariable",
        recommended: false,
    }
}

/// Signal struct emitted for each variable declaration the rule can inline
pub(crate) struct State {
    /// List of references to the variable
    references: Vec<Reference>,
    /// Initializer expression for the variable to be inlined
    expression: JsAnyExpression,
}

impl Rule for InlineVariable {
    type Query = Semantic<JsVariableDeclarator>;
    type State = State;
    type Signals = Option<Self::State>;
    type Options = ();

    fn run(ctx: &RuleContext<Self>) -> Option<Self::State> {
        let semantic_model = ctx.model();
        let declarator = ctx.query();

        let id = declarator.id().ok()?;
        let binding = match id {
            JsAnyBindingPattern::JsAnyBinding(JsAnyBinding::JsIdentifierBinding(binding)) => {
                binding
            }
            JsAnyBindingPattern::JsAnyBinding(JsAnyBinding::JsUnknownBinding(_))
            | JsAnyBindingPattern::JsArrayBindingPattern(_)
            | JsAnyBindingPattern::JsObjectBindingPattern(_) => return None,
        };

        // Do not inline if the initializer is not inlinable

        let initializer = declarator.initializer()?;
        let expr = initializer.expression().ok()?;
        match expr {
            JsAnyExpression::JsArrowFunctionExpression(_)
            | JsAnyExpression::JsFunctionExpression(_)
            | JsAnyExpression::JsClassExpression(_)
            | JsAnyExpression::JsAssignmentExpression(_) => return None,
            _ => {}
        }

        // Do not inline if there is a write

        let mut references = Vec::new();
        for reference in binding.all_references(semantic_model) {
            if reference.is_write() {
                return None;
            }

            references.push(reference);
        }

        // Inline variable

        let expression = initializer.expression().ok()?;
        Some(State {
            references,
            expression,
        })
    }

    fn action(ctx: &RuleContext<Self>, state: &Self::State) -> Option<JsRuleAction> {
        let node = ctx.query();
        let mut mutation = ctx.root().begin();
        let State {
            references,
            expression,
        } = state;

        remove_declarator(&mut mutation, node);

        for reference in references {
            let node = reference
                .node()
                .parent()?
                .cast::<JsIdentifierExpression>()?;

            mutation.replace_node(
                JsAnyExpression::JsIdentifierExpression(node),
                expression.clone(),
            );
        }

        Some(JsRuleAction {
            category: ActionCategory::Refactor(RefactorKind::Inline),
            applicability: Applicability::Always,
            message: markup! { "Inline variable" }.to_owned(),
            mutation,
        })
    }
}
