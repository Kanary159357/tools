use crate::prelude::*;
use rome_formatter::write;

use crate::utils::FormatStatementSemicolon;

use rome_js_syntax::JsExportFromClause;
use rome_js_syntax::JsExportFromClauseFields;

#[derive(Debug, Clone, Default)]
pub struct FormatJsExportFromClause;

impl FormatNodeRule<JsExportFromClause> for FormatJsExportFromClause {
    fn fmt_fields(&self, node: &JsExportFromClause, f: &mut JsFormatter) -> FormatResult<()> {
        let JsExportFromClauseFields {
            star_token,
            export_as,
            from_token,
            source,
            assertion,
            semicolon_token,
        } = node.as_fields();

        write!(f, [star_token.format(), space(),])?;

        if let Some(export_as) = export_as {
            write!(f, [export_as.format(), space()])?;
        }

        write!(f, [from_token.format(), space(), source.format()])?;

        if let Some(assertion) = assertion {
            write!(f, [space(), assertion.format()])?;
        }

        FormatStatementSemicolon::new(semicolon_token.as_ref()).fmt(f)
    }
}
