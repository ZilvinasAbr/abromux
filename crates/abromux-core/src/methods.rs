//! Control methods by what they need: the workspaces' model or the app's
//! views (docs/server-plan.md, phase 2). Shared by the server, which takes
//! model methods, and the CLI, which describes them.

/// Whether control method `method` is about the workspaces' model (true) or
/// needs only the app's views (browser automation, input, focus, reading a
/// screen, questions to a person). The app's `api::kind` is this.
pub fn is_model_method(method: &str) -> bool {
    let (ns, verb) = method.split_once('.').unwrap_or((method, ""));
    match ns {
        "workspace" | "group" | "status" | "progress" | "log" | "notification" | "link"
        | "forward" | "agent" | "tab" | "system" | "session" => true,
        "pane" => verb != "zoom",
        "surface" => matches!(
            verb,
            "list"
                | "split"
                | "create"
                | "close"
                | "rename"
                | "respawn"
                | "move"
                | "reorder"
                | "split_off"
                | "move_to_new_workspace"
                | "clear_history"
                | "reclaim"
                | "restream"
        ),
        _ => false,
    }
}
