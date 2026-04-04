//! Smoke test — verifies core types are accessible and constructible.

use mage_core::session::SessionHandle;
use mage_core::types::{Message, ToolResult};

#[test]
fn tool_result_success() {
    let r = ToolResult::success("ok");
    assert!(!r.is_error);
    assert_eq!(r.content.len(), 1);
}

#[test]
fn tool_result_failure() {
    let r = ToolResult::failure("bad");
    assert!(r.is_error);
}

#[test]
fn message_user_text() {
    let m = Message::user_text("hello");
    assert_eq!(m.role_name(), "user");
    assert!(!m.is_ephemeral());
}

#[test]
fn session_handle_test() {
    let h = SessionHandle::test_handle();
    assert!(h.is_idle());
}
