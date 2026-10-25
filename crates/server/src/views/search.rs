//! Shared collection and embedded-table search controls and query options.
use super::{escape, table};
use std::collections::HashMap;
use structured_editor_core::SearchOptions;

pub fn options(params: &HashMap<String, String>, prefix: &str) -> SearchOptions {
    SearchOptions {
        regex: params
            .get(&format!("{prefix}regex"))
            .is_some_and(|v| v == "1"),
        case_sensitive: params
            .get(&format!("{prefix}case_sensitive"))
            .is_some_and(|v| v == "1"),
    }
}

pub fn form(
    base: &str,
    params: &HashMap<String, String>,
    prefix: &str,
    scope: &str,
    placeholder: &str,
    pattern_error: Option<&str>,
    extra: &str,
) -> String {
    form_with_submit(
        base,
        params,
        prefix,
        scope,
        placeholder,
        pattern_error,
        extra,
        table::GET_FORM,
    )
}

pub fn form_with_submit(
    base: &str,
    params: &HashMap<String, String>,
    prefix: &str,
    scope: &str,
    placeholder: &str,
    pattern_error: Option<&str>,
    extra: &str,
    submit: &str,
) -> String {
    let key = |suffix: &str| format!("{prefix}{suffix}");
    let options = options(params, prefix);
    let scope = escape(scope);
    format!(
        "<form class=\"search-form\" data-regex-search action=\"{}\" {}>{}<div class=\"search-input has-options\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><label class=\"sr-only\" for=\"{scope}-input\">{ui_text_0}</label><input id=\"{scope}-input\" data-search-query aria-invalid=\"{}\" aria-describedby=\"{scope}-error\" type=\"search\" name=\"{}\" value=\"{}\" placeholder=\"{}\"><input type=\"hidden\" data-search-case name=\"{}\" value=\"{}\"><input type=\"hidden\" data-search-regex name=\"{}\" value=\"{}\"><span class=\"search-options\"><button type=\"button\" class=\"search-toggle\" data-case-toggle aria-label=\"{ui_text_2}\" aria-pressed=\"{}\" title=\"{ui_text_2}\"><span aria-hidden=\"true\">Aa</span></button><button type=\"button\" class=\"search-toggle\" data-regex-toggle aria-label=\"{ui_text_3}\" aria-pressed=\"{}\" title=\"{ui_text_3}\"><span aria-hidden=\"true\">.*</span></button></span></div><button>{ui_text_1}</button>{extra}</form>",
        escape(base),
        submit,
        table::hidden(
            params,
            &[
                &key("q"),
                &key("regex"),
                &key("case_sensitive"),
                &key("offset")
                , "table_anchor"
            ]
        ),
        pattern_error.is_some(),
        escape(&key("q")),
        escape(params.get(&key("q")).map(String::as_str).unwrap_or("")),
        escape(placeholder),
        escape(&key("case_sensitive")),
        if options.case_sensitive { "1" } else { "0" },
        escape(&key("regex")),
        if options.regex { "1" } else { "0" },
        options.case_sensitive,
        options.regex, ui_text_0 = crate::views::escape(&crate::i18n::t("Search")), ui_text_1 = crate::views::escape(&crate::i18n::t("Search")), ui_text_2 = crate::views::escape(&crate::i18n::t("Match case")), ui_text_3 = crate::views::escape(&crate::i18n::t("Use a regular expression")))
}

pub fn validation(scope: &str, pattern_error: Option<&str>) -> String {
    format!(
        "<span id=\"{}-error\" class=\"search-pattern-error\" role=\"status\">{}</span>",
        escape(scope),
        escape(pattern_error.unwrap_or(""))
    )
}
