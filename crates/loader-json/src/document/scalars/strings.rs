//! The built-in JSON reader uses the adapter decoder after framing has
//! already validated the source bytes.

pub(super) use super::json_string::{
    with_next_quoted_text_validated as with_next_quoted_text,
    with_quoted_text_validated as with_quoted_text,
};
