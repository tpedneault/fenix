//! A test extension for service callers (API 1.6). Installed under any
//! id, it serves any name under it, by the name's last part:
//!
//! - `who`: who called it (`called_by`), as `extension:<id>`, `fenix`,
//!   `agent-tool`, `agent-resource`, `agent-prompt`, `control` or `none`.
//! - `who_later`: the same, answered once a timer goes off -- read in
//!   `serve`, kept with the call.
//! - `relay`: the payload is `<service> <rest>`; it calls that service
//!   with `<rest>` and answers `<who called it> | <that one's answer>`.
//! - `gated`: counts that it ran, and says so on its output page.
//! - `count`: how many times `gated` ran.
//! - anything else (`notebook.capture`, as Fenix's control channel asks
//!   the notebook): `who`.
//!
//! It says on its output page who called each (`<name> called by
//! <who>`), and who `called_by` says outside `serve`: as it starts, and
//! when a reply comes.

use std::collections::HashMap;

use fenix_extension_api::{answer, call_service, called_by, export, host, info, Event, Extension, ServiceCaller};

fn said(caller: Option<ServiceCaller>) -> String {
    match caller {
        None => "none".into(),
        Some(ServiceCaller::Extension(id)) => format!("extension:{id}"),
        Some(ServiceCaller::Fenix) => "fenix".into(),
        Some(ServiceCaller::AgentTool) => "agent-tool".into(),
        Some(ServiceCaller::AgentResource) => "agent-resource".into(),
        Some(ServiceCaller::AgentPrompt) => "agent-prompt".into(),
        Some(ServiceCaller::Control) => "control".into(),
    }
}

#[derive(Default)]
struct Callers {
    /// A `who_later` waiting for its timer: the call, and who called it.
    later: HashMap<u64, (u64, String)>,
    /// A `relay` waiting for the service it called: the call, and who
    /// called it.
    relayed: HashMap<u64, (u64, String)>,
    gated: u32,
}

impl Extension for Callers {
    fn activate(&mut self) -> Result<(), String> {
        info!("called-by in activate: {}", said(called_by()));
        Ok(())
    }

    fn serve(&mut self, call: u64, name: &str, payload: &str) -> Option<Result<String, String>> {
        let caller = said(called_by());
        info!("{name} called by {caller}");
        match name.rsplit('.').next().unwrap_or_default() {
            "who_later" => {
                let timer = host::set_timer(30, false);
                self.later.insert(timer, (call, caller));
                None
            }
            "relay" => {
                let (next, rest) = payload.split_once(' ').unwrap_or((payload, ""));
                let asked = call_service(next, rest);
                self.relayed.insert(asked, (call, caller));
                None
            }
            "gated" => {
                self.gated += 1;
                info!("gated ran, for {caller}");
                Some(Ok(format!("gated, for {caller}")))
            }
            "count" => Some(Ok(self.gated.to_string())),
            _ => Some(Ok(caller)),
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Timer(timer) => {
                if let Some((call, caller)) = self.later.remove(&timer) {
                    answer(call, Ok(caller));
                }
            }
            Event::ServiceReply((asked, reply)) => {
                info!("called-by in handle: {}", said(called_by()));
                if let Some((call, caller)) = self.relayed.remove(&asked) {
                    answer(call, reply.map(|r| format!("{caller} | {r}")).map_err(|e| format!("{caller} | {e}")));
                }
            }
            _ => {}
        }
    }
}

export!(Callers);
