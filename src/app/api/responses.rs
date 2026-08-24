use crate::api::schema::{ErrorBody, ErrorResponse, ResponseResult, SuccessResponse};

pub(super) fn encode_success(id: String, result: ResponseResult) -> String {
    serde_json::to_string(&SuccessResponse { id, result }).expect(
        "SuccessResponse is id + ResponseResult of strings/numbers/nested plain structs; serde_json only fails on non-finite floats or non-string map keys, neither possible here",
    )
}

pub(super) fn encode_error(id: String, code: &str, message: impl Into<String>) -> String {
    encode_error_body(
        id,
        ErrorBody {
            code: code.into(),
            message: message.into(),
        },
    )
}

pub(super) fn encode_error_body(id: String, error: ErrorBody) -> String {
    serde_json::to_string(&ErrorResponse { id, error }).expect(
        "ErrorResponse is id + ErrorBody of plain strings; serde_json only fails on non-finite floats or non-string map keys, neither possible here",
    )
}
