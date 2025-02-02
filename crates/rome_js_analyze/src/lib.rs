use control_flow::make_visitor;
use rome_analyze::{
    AnalysisFilter, Analyzer, AnalyzerContext, AnalyzerOptions, AnalyzerSignal, ControlFlow,
    InspectMatcher, LanguageRoot, MatchQueryParams, MetadataRegistry, Phases, RuleAction,
    RuleRegistry, ServiceBag, SuppressionCommentEmitterPayload, SuppressionKind, SyntaxVisitor,
};
use rome_diagnostics::{category, FileId};
use rome_js_factory::make::{jsx_expression_child, token};
use rome_js_syntax::{
    suppression::parse_suppression_comment, JsLanguage, JsSyntaxToken, JsxAnyChild, T,
};
use rome_rowan::{AstNode, TokenAtOffset, TriviaPieceKind};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, error::Error};

mod analyzers;
mod assists;
mod ast_utils;
mod control_flow;
pub mod globals;
mod react;
mod registry;
mod semantic_analyzers;
mod semantic_services;
mod syntax;
pub mod utils;

pub use crate::registry::visit_registry;
use crate::semantic_services::{SemanticModelBuilderVisitor, SemanticModelVisitor};
use crate::utils::batch::JsBatchMutation;

pub(crate) type JsRuleAction = RuleAction<JsLanguage>;

/// Return the static [MetadataRegistry] for the JS analyzer rules
pub fn metadata() -> &'static MetadataRegistry {
    lazy_static::lazy_static! {
        static ref METADATA: MetadataRegistry = {
            let mut metadata = MetadataRegistry::default();
            visit_registry(&mut metadata);
            metadata
        };
    }

    &METADATA
}

/// Run the analyzer on the provided `root`: this process will use the given `filter`
/// to selectively restrict analysis to specific rules / a specific source range,
/// then call `emit_signal` when an analysis rule emits a diagnostic or action.
/// Additionally, this function takes a `inspect_matcher` function that can be
/// used to inspect the "query matches" emitted by the analyzer before they are
/// processed by the lint rules registry
pub fn analyze_with_inspect_matcher<'a, V, F, B>(
    file_id: FileId,
    root: &LanguageRoot<JsLanguage>,
    filter: AnalysisFilter,
    inspect_matcher: V,
    options: &'a AnalyzerOptions,
    mut emit_signal: F,
) -> Option<B>
where
    V: FnMut(&MatchQueryParams<JsLanguage>) + 'a,
    F: FnMut(&dyn AnalyzerSignal<JsLanguage>) -> ControlFlow<B> + 'a,
    B: 'a,
{
    fn parse_linter_suppression_comment(text: &str) -> Vec<SuppressionKind> {
        parse_suppression_comment(text)
            .flat_map(|comment| comment.categories)
            .filter_map(|(key, value)| {
                if key == category!("lint") {
                    if let Some(value) = value {
                        Some(SuppressionKind::MaybeLegacy(value))
                    } else {
                        Some(SuppressionKind::Everything)
                    }
                } else {
                    let category = key.name();
                    category.strip_prefix("lint/").map(SuppressionKind::Rule)
                }
            })
            .collect()
    }

    let mut registry = RuleRegistry::builder(&filter);
    visit_registry(&mut registry);

    let mut analyzer = Analyzer::new(
        metadata(),
        InspectMatcher::new(registry.build(), inspect_matcher),
        parse_linter_suppression_comment,
        apply_suppression_comment,
        &mut emit_signal,
    );
    analyzer.add_visitor(Phases::Syntax, make_visitor());
    analyzer.add_visitor(Phases::Syntax, SyntaxVisitor::default());
    analyzer.add_visitor(Phases::Syntax, SemanticModelBuilderVisitor::new(root));

    analyzer.add_visitor(Phases::Semantic, SemanticModelVisitor);
    analyzer.add_visitor(Phases::Semantic, SyntaxVisitor::default());

    analyzer.run(AnalyzerContext {
        file_id,
        root: root.clone(),
        range: filter.range,
        services: ServiceBag::default(),
        options,
    })
}

/// Run the analyzer on the provided `root`: this process will use the given `filter`
/// to selectively restrict analysis to specific rules / a specific source range,
/// then call `emit_signal` when an analysis rule emits a diagnostic or action
pub fn analyze<'a, F, B>(
    file_id: FileId,
    root: &LanguageRoot<JsLanguage>,
    filter: AnalysisFilter,
    options: &'a AnalyzerOptions,
    emit_signal: F,
) -> Option<B>
where
    F: FnMut(&dyn AnalyzerSignal<JsLanguage>) -> ControlFlow<B> + 'a,
    B: 'a,
{
    analyze_with_inspect_matcher(file_id, root, filter, |_| {}, options, emit_signal)
}

#[cfg(test)]
mod tests {
    use rome_analyze::{AnalyzerOptions, Never, RuleCategories, RuleFilter};
    use rome_console::fmt::{Formatter, Termcolor};
    use rome_console::{markup, Markup};
    use rome_diagnostics::termcolor::NoColor;
    use rome_diagnostics::{category, location::FileId};
    use rome_diagnostics::{Diagnostic, DiagnosticExt, PrintDiagnostic, Severity};
    use rome_js_parser::parse;
    use rome_js_syntax::{SourceType, TextRange, TextSize};

    use crate::{analyze, AnalysisFilter, ControlFlow};

    #[ignore]
    #[test]
    fn quick_test() {
        fn markup_to_string(markup: Markup) -> String {
            let mut buffer = Vec::new();
            let mut write = Termcolor(NoColor::new(&mut buffer));
            let mut fmt = Formatter::new(&mut write);
            fmt.write_markup(markup).unwrap();

            String::from_utf8(buffer).unwrap()
        }

        const SOURCE: &str = r#"function f() {
            return (
                <div
                ><img /></div>
            )
        };
        f();
        "#;

        let parsed = parse(SOURCE, FileId::zero(), SourceType::jsx());

        let mut error_ranges: Vec<TextRange> = Vec::new();
        let options = AnalyzerOptions::default();
        let _rule_filter = RuleFilter::Rule("correctness", "flipBinExp");
        analyze(
            FileId::zero(),
            &parsed.tree(),
            AnalysisFilter {
                // enabled_rules: Some(slice::from_ref(&rule_filter)),
                ..AnalysisFilter::default()
            },
            &options,
            |signal| {
                dbg!("here");
                if let Some(mut diag) = signal.diagnostic() {
                    diag.set_severity(Severity::Warning);
                    error_ranges.push(diag.location().unwrap().span.unwrap());
                    let error = diag.with_file_path("ahahah").with_file_source_code(SOURCE);
                    let text = markup_to_string(markup! {
                        {PrintDiagnostic(&error)}
                    });
                    eprintln!("{text}");
                }

                for action in signal.actions() {
                    let new_code = action.mutation.commit();
                    eprintln!("{new_code}");
                }

                ControlFlow::<Never>::Continue(())
            },
        );

        assert_eq!(error_ranges.as_slice(), &[]);
    }

    #[test]
    fn suppression() {
        const SOURCE: &str = "
            function checkSuppressions1(a, b) {
                a == b;
                // rome-ignore lint/correctness:whole group
                a == b;
                // rome-ignore lint/correctness/noDoubleEquals: single rule
                a == b;
                /* rome-ignore lint/correctness/useWhile: multiple block comments */ /* rome-ignore lint/correctness/noDoubleEquals: multiple block comments */
                a == b;
                // rome-ignore lint/correctness/useWhile: multiple line comments
                // rome-ignore lint/correctness/noDoubleEquals: multiple line comments
                a == b;
                a == b;
            }

            // rome-ignore lint/correctness/noDoubleEquals: do not suppress warning for the whole function
            function checkSuppressions2(a, b) {
                a == b;
            }

            function checkSuppressions3(a, b) {
                a == b;
                // rome-ignore lint(correctness): whole group
                a == b;
                // rome-ignore lint(correctness/noDoubleEquals): single rule
                a == b;
                /* rome-ignore lint(correctness/useWhile): multiple block comments */ /* rome-ignore lint(correctness/noDoubleEquals): multiple block comments */
                a == b;
                // rome-ignore lint(correctness/useWhile): multiple line comments
                // rome-ignore lint(correctness/noDoubleEquals): multiple line comments
                a == b;
                a == b;
            }

            // rome-ignore lint(correctness/noDoubleEquals): do not suppress warning for the whole function
            function checkSuppressions4(a, b) {
                a == b;
            }
        ";

        let parsed = parse(SOURCE, FileId::zero(), SourceType::js_module());

        let mut error_ranges: Vec<TextRange> = Vec::new();
        let mut warn_ranges: Vec<TextRange> = Vec::new();

        let options = AnalyzerOptions::default();
        analyze(
            FileId::zero(),
            &parsed.tree(),
            AnalysisFilter::default(),
            &options,
            |signal| {
                if let Some(mut diag) = signal.diagnostic() {
                    diag.set_severity(Severity::Warning);

                    let span = diag.get_span();
                    let error = diag
                        .with_file_path(FileId::zero())
                        .with_file_source_code(SOURCE);

                    let code = error.category().unwrap();
                    if code == category!("lint/correctness/noDoubleEquals") {
                        error_ranges.push(span.unwrap());
                    }

                    if code == category!("suppressions/deprecatedSyntax") {
                        assert!(signal.actions().len() > 0);
                        warn_ranges.push(span.unwrap());
                    }
                }

                ControlFlow::<Never>::Continue(())
            },
        );

        assert_eq!(
            error_ranges.as_slice(),
            &[
                TextRange::new(TextSize::from(67), TextSize::from(69)),
                TextRange::new(TextSize::from(651), TextSize::from(653)),
                TextRange::new(TextSize::from(845), TextSize::from(847)),
                TextRange::new(TextSize::from(932), TextSize::from(934)),
                TextRange::new(TextSize::from(1523), TextSize::from(1525)),
                TextRange::new(TextSize::from(1718), TextSize::from(1720)),
            ]
        );

        assert_eq!(
            warn_ranges.as_slice(),
            &[
                TextRange::new(TextSize::from(954), TextSize::from(999)),
                TextRange::new(TextSize::from(1040), TextSize::from(1100)),
                TextRange::new(TextSize::from(1141), TextSize::from(1210)),
                TextRange::new(TextSize::from(1211), TextSize::from(1286)),
                TextRange::new(TextSize::from(1327), TextSize::from(1392)),
                TextRange::new(TextSize::from(1409), TextSize::from(1480)),
                TextRange::new(TextSize::from(1556), TextSize::from(1651)),
            ]
        );
    }

    #[test]
    fn suppression_syntax() {
        const SOURCE: &str = "
            // rome-ignore lint/correctness/noDoubleEquals: single rule
            a == b;
        ";

        let parsed = parse(SOURCE, FileId::zero(), SourceType::js_module());

        let filter = AnalysisFilter {
            categories: RuleCategories::SYNTAX,
            ..AnalysisFilter::default()
        };

        let options = AnalyzerOptions::default();
        analyze(FileId::zero(), &parsed.tree(), filter, &options, |signal| {
            if let Some(diag) = signal.diagnostic() {
                let code = diag.category().unwrap();
                if code != category!("suppressions/unused") {
                    panic!("unexpected diagnostic {code:?}");
                }
            }

            ControlFlow::<Never>::Continue(())
        });
    }
}

/// Series of errors encountered when running rules on a file
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum RuleError {
    /// The rule with the specified name replaced the root of the file with a node that is not a valid root for that language.
    ReplacedRootWithNonRootError {
        rule_name: Option<(Cow<'static, str>, Cow<'static, str>)>,
    },
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            RuleError::ReplacedRootWithNonRootError {
                rule_name: Some((group, rule)),
            } => {
                std::write!(
                    fmt,
                    "the rule '{group}/{rule}' replaced the root of the file with a non-root node."
                )
            }
            RuleError::ReplacedRootWithNonRootError { rule_name: None } => {
                std::write!(
                    fmt,
                    "a code action replaced the root of the file with a non-root node."
                )
            }
        }
    }
}

impl Error for RuleError {}

/// We now try to "guess" the token where to apply the suppression comment.
/// Considering that the detection of suppression comments in the linter is "line based", we start
/// querying the node covered by the text range of the diagnostic, until we find the first token that has a newline
/// among its leading trivia.
///
/// If we're not able to find any token, it means that the range is
/// placed at row 1, so we take the root itself.
fn apply_suppression_comment(payload: SuppressionCommentEmitterPayload<JsLanguage>) {
    let SuppressionCommentEmitterPayload {
        token_offset,
        mutation,
        suppression_text,
        diagnostic_text_range,
    } = payload;
    let current_token = match token_offset {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(token) => Some(match find_token_with_newline(token.clone()) {
            None => token,
            Some(token) => token,
        }),
        TokenAtOffset::Between(left_token, right_token) => {
            let chosen_token = if right_token.text_range().start() == diagnostic_text_range.start()
            {
                right_token
            } else {
                left_token
            };
            Some(chosen_token)
        }
    };
    if let Some(current_token) = current_token {
        if let Some(element) = current_token.ancestors().find_map(JsxAnyChild::cast) {
            let jsx_comment = jsx_expression_child(
                token(T!['{']).with_trailing_trivia([(
                    TriviaPieceKind::SingleLineComment,
                    format!("/* {} */", suppression_text).as_str(),
                )]),
                token(T!['}']),
            );
            mutation.add_jsx_element_before_element(
                &element,
                &JsxAnyChild::JsxExpressionChild(jsx_comment.build()),
            );
        } else {
            let new_token = current_token.with_leading_trivia([
                (TriviaPieceKind::Newline, "\n"),
                (
                    TriviaPieceKind::SingleLineComment,
                    format!("// {} ", suppression_text).as_str(),
                ),
                (TriviaPieceKind::Newline, "\n"),
            ]);
            mutation.replace_token_transfer_trivia(current_token, new_token);
        }
    }
}

/// It checks if the current token has leading trivia newline. If not, it
/// it peeks the previous token and recursively call itself
fn find_token_with_newline(token: JsSyntaxToken) -> Option<JsSyntaxToken> {
    let mut current_token = token;
    loop {
        let trivia = current_token.leading_trivia();
        if trivia.pieces().any(|trivia| trivia.is_newline())
            || current_token.text_trimmed().contains('\n')
        {
            break;
        } else if let Some(token) = current_token.prev_token() {
            current_token = token;
            continue;
        } else {
            return None;
        }
    }

    Some(current_token)
}
