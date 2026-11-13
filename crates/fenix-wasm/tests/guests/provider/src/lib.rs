//! A test provider of a made-up runner, `fake-test-runner`, through every
//! export of API 1.6 and its import, as a language pack writes one.
//!
//! - Its scan reads `# suite NAME`, `# test NAME`, `# skip NAME REASON`
//!   and `# each NAME` lines; a file whose first line is `#limits`,
//!   `#over` or `#many` answers a test at every limit, one over each, or
//!   10,001 tests.
//! - Its plans run `fake-test-runner`: one step to list, two for all
//!   (`list-after`), one a file asked for and one for the rest.
//! - Its reader takes `PASS id ms`, `FAIL id file:line message`, `SKIP id
//!   why`, `ERROR id why`, `START id`, `OUT text`, `BROKEN id message`,
//!   `CRASH id message`, `FOUND id parent`, `MANY n` and `HUGE`, a line at
//!   a time, a cut line joined to the next.
//! - Provider `trap` panics in every export, `spin` never returns from
//!   one; `noplan` has no plan,
//!   `denied` plans a program it may not run, `biglaunch` launches with
//!   2 MiB of attributes.
//! - Its launches: a test with `data` now; a case, and a test whose name
//!   starts `build`, after a build (`fake-test-runner --build ID`) whose
//!   `BUILT path` line names the binary the second ask launches (asked
//!   again with no such line, it asks to build again). A launch's
//!   attributes take `/project/fake-debug.json`'s when it's there (a
//!   test's `debugServer`). Anything else can't be debugged.

use std::collections::HashMap;

use fenix_extension_api::tests::{self, Ask, Ended, Found, Kind, Launch, Line, Location, Outcome, Plan, Report, Step, StepEnd};
use fenix_extension_api::{export, info, Command, Extension, LaunchConfig};

#[derive(Default)]
struct Reader {
    /// A line cut at 64 KiB, until its rest comes.
    pending: String,
    lines: u32,
}

#[derive(Default)]
struct Provider {
    /// A reader per step of a run, made at its first batch, dropped at its
    /// end.
    readers: HashMap<(u64, u32), Reader>,
    /// The runs it asked to build first, and the binary each build said.
    built: HashMap<u64, Option<String>>,
}

fn trap(provider: &str) {
    match provider {
        "trap" => panic!("the provider trapped"),
        "spin" => loop {
            std::hint::spin_loop();
        },
        _ => {}
    }
}

fn found(id: String, parent: Option<String>, label: &str, kind: Kind) -> Found {
    Found { id, parent, label: label.into(), kind, ..Found::default() }
}

impl Provider {
    fn read(&mut self, run: u64, step: u32, line: Line) -> Vec<Report> {
        let reader = self.readers.entry((run, step)).or_default();
        reader.lines += 1;
        if line.cut {
            reader.pending.push_str(&line.text);
            return Vec::new();
        }
        let text = std::mem::take(&mut reader.pending) + &line.text;
        let (word, rest) = text.split_once(' ').unwrap_or((text.as_str(), ""));
        if word == "BUILT" {
            if let Some(built) = self.built.get_mut(&run) {
                *built = Some(rest.to_string());
            }
            return Vec::new();
        }
        let (id, more) = rest.split_once(' ').unwrap_or((rest, ""));
        let ended = |outcome, message: Option<String>, at: Option<Location>| Report::Ended(Ended { id: id.into(), outcome, duration_ms: None, message, at, text: Some(text.clone()) });
        match word {
            "PASS" => vec![Report::Ended(Ended { id: id.into(), outcome: Outcome::Passed, duration_ms: more.trim_end_matches("ms").parse().ok(), message: None, at: None, text: None })],
            "FAIL" => {
                let (place, message) = more.split_once(' ').unwrap_or((more, ""));
                let at = place.rsplit_once(':').and_then(|(path, line)| Some(Location { path: path.into(), line: line.parse().ok()?, column: None }));
                vec![ended(Outcome::Failed, Some(message.into()), at)]
            }
            "SKIP" => vec![ended(Outcome::Skipped, Some(more.into()), None)],
            "ERROR" => vec![ended(Outcome::Errored, Some(more.into()), None)],
            "START" => vec![Report::Started(id.into())],
            "OUT" => vec![Report::Output { id: None, text: rest.into() }],
            "BROKEN" => vec![Report::Broken { id: Some(id.into()), message: more.into(), at: None }],
            "CRASH" => vec![Report::Crashed { id: Some(id.into()), message: more.into() }],
            "FOUND" => vec![Report::Found(found(id.into(), Some(more.into()), id.rsplit('/').next().unwrap_or(id), Kind::Case))],
            "MANY" => (0..id.parse::<u32>().unwrap_or(0)).map(|i| Report::Started(format!("t{i}"))).collect(),
            "HUGE" => vec![Report::Output { id: Some("huge".into()), text: format!("start{}", "e".repeat(70 << 10)) }],
            _ => Vec::new(),
        }
    }
}

impl Extension for Provider {
    fn activate(&mut self) -> Result<(), String> {
        // Its reporter, which its steps need.
        std::fs::write("/data/reporter.txt", "the reporter").map_err(|e| e.to_string())
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        let said = match id {
            "provider.set" => tests::provider("provider", "fake-test-runner 2.0", None),
            "provider.hide" => tests::provider("provider", "", None),
            "provider.problem" => tests::provider("provider.other", "fake-test-runner", Some("it isn't installed")),
            "provider.unknown" => tests::provider("python", "pytest", None),
            "provider.long" => tests::provider("provider.other", &"r".repeat(300), Some(&"p".repeat(5000))),
            _ => return Err(format!("no command {id}")),
        };
        info!("{id}: {said:?}");
        Ok(())
    }

    fn tests_scan(&mut self, provider: &str, path: &str, text: &str) -> Vec<Found> {
        trap(provider);
        let file = path.strip_prefix("/project/").unwrap_or(path).to_string();
        let mut out = vec![Found { path: Some(path.into()), ..found(file.clone(), None, file.rsplit('/').next().unwrap_or(&file), Kind::File) }];
        match text.lines().next() {
            Some("#limits") | Some("#over") => {
                let over = usize::from(text.starts_with("#over"));
                out.push(Found {
                    id: "i".repeat(1024 + over),
                    parent: Some("p".repeat(1024 + over)),
                    label: "é".repeat(256 + over),
                    kind: Kind::Case,
                    path: Some("f".repeat(4096 + over)),
                    line: Some(u32::MAX),
                    end_line: Some(u32::MAX),
                    skip: Some("s".repeat(1024 + over)),
                    template: true,
                    data: Some("d".repeat(4096 + over)),
                });
                return out;
            }
            Some("#many") => return (0..10_001).map(|i| found(format!("{file}::t{i}"), Some(file.clone()), "t", Kind::Test)).collect(),
            _ => {}
        }
        let mut suite: Option<String> = None;
        for (n, line) in text.lines().enumerate() {
            let Some(rest) = line.trim().strip_prefix("# ") else { continue };
            let (word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            let (name, why) = rest.split_once(' ').unwrap_or((rest, ""));
            let parent = suite.clone().unwrap_or_else(|| file.clone());
            let mut test = Found { line: Some(n as u32), ..found(format!("{file}::{name}"), Some(parent), name, Kind::Test) };
            match word {
                "suite" => {
                    test.kind = Kind::Suite;
                    test.parent = Some(file.clone());
                    suite = Some(test.id.clone());
                }
                "test" => {}
                "skip" => test.skip = Some(why.into()),
                "each" => {
                    test.template = true;
                    test.data = Some(format!("{{\"template\":\"{name}\"}}"));
                }
                _ => continue,
            }
            out.push(test);
        }
        out
    }

    fn tests_plan(&mut self, provider: &str, run: u64, ask: Ask) -> Result<Plan, String> {
        trap(provider);
        let step = |args: Vec<String>, label: &str, covers: Vec<String>| Step { command: Command::new("fake-test-runner").args(args), label: label.into(), covers, needs: Vec::new() };
        match provider {
            "noplan" => return Err("n".repeat(5000)),
            "denied" => return Ok(Plan { steps: vec![Step { command: Command::new("rm").arg("-rf"), ..step(Vec::new(), "x", Vec::new()) }], list_after: false }),
            _ => {}
        }
        match ask {
            Ask::Discover => Ok(Plan { steps: vec![Step { command: Command::new("fake-test-runner").arg("--list").cwd("/data"), ..step(Vec::new(), "list", Vec::new()) }], list_after: false }),
            Ask::RunAll => Ok(Plan {
                steps: vec![
                    Step { command: Command::new("fake-test-runner").arg("--unit").arg("${data}/ids.txt").env("FAKE", "${data}/x"), needs: vec!["/data/reporter.txt".into()], ..step(Vec::new(), "unit", vec!["unit".into()]) },
                    Step { needs: vec!["/data/missing.txt".into(), "elsewhere/x".into()], ..step(vec!["--integration".into()], "integration", Vec::new()) },
                ],
                list_after: true,
            }),
            Ask::Run(found) => {
                let asked: Vec<String> = found.iter().map(|f| format!("{}{}", f.id, f.data.as_deref().map(|d| format!("={d}")).unwrap_or_default())).collect();
                info!("run {run} asked {}: {}", found.len(), asked.join(", "));
                let mut steps: Vec<Step> = Vec::new();
                let mut rest = Vec::new();
                for f in &found {
                    match f.kind {
                        Kind::File => steps.push(step(vec![f.id.clone()], &f.label, vec![f.id.clone()])),
                        _ if f.parent.as_ref().is_some_and(|p| steps.iter().any(|s| s.covers.contains(p))) => steps.iter_mut().find(|s| f.parent.as_ref().is_some_and(|p| s.covers.contains(p))).into_iter().for_each(|s| s.command.args.push(f.id.clone())),
                        _ => rest.push(f.id.clone()),
                    }
                }
                if !rest.is_empty() {
                    steps.push(step(rest, "the rest", Vec::new()));
                }
                Ok(Plan { steps, list_after: false })
            }
        }
    }

    fn tests_read(&mut self, provider: &str, run: u64, step: u32, lines: Vec<Line>) -> Vec<Report> {
        trap(provider);
        lines.into_iter().flat_map(|line| self.read(run, step, line)).collect()
    }

    fn tests_step_ended(&mut self, provider: &str, run: u64, step: u32, end: StepEnd) -> Vec<Report> {
        trap(provider);
        let reader = self.readers.remove(&(run, step)).unwrap_or_default();
        let mut out = Vec::new();
        if !reader.pending.is_empty() {
            out.push(Report::Output { id: None, text: reader.pending });
        }
        out.push(Report::Output { id: None, text: format!("step {step} of run {run} ended {end:?} after {} lines", reader.lines) });
        out
    }

    fn tests_launch(&mut self, provider: &str, run: u64, test: Found) -> Result<Launch, String> {
        trap(provider);
        if provider == "biglaunch" {
            return Ok(Launch::Launch(LaunchConfig::launch("big", &["fake"]).attributes(&fenix_extension_api::serde_json::json!({ "x": "a".repeat(2 << 20) }))));
        }
        // A test's own attributes, from the project: an adapter listening.
        let attributes = |mut base: fenix_extension_api::serde_json::Map<String, fenix_extension_api::serde_json::Value>| {
            let extra = std::fs::read_to_string("/project/fake-debug.json").ok().and_then(|t| fenix_extension_api::serde_json::from_str::<fenix_extension_api::serde_json::Map<String, fenix_extension_api::serde_json::Value>>(&t).ok());
            base.extend(extra.unwrap_or_default());
            fenix_extension_api::serde_json::Value::Object(base)
        };
        let build = |id: &str| Plan { steps: vec![Step { command: Command::new("fake-test-runner").arg("--build").arg(id), label: "build".into(), covers: Vec::new(), needs: Vec::new() }], list_after: false };
        // Asked again after its build: the binary it said, or another build.
        if let Some(built) = self.built.remove(&run) {
            return match built {
                Some(binary) => Ok(Launch::Launch(LaunchConfig::launch(&test.label, &["fake"]).program(&binary).arg(test.id).arg("--exact").cwd("/project").attributes(&attributes(Default::default())))),
                None => Ok(Launch::BuildFirst(build(&test.id))),
            };
        }
        match (test.kind, test.data) {
            (Kind::Case, _) => Ok(Launch::BuildFirst(build(&test.id))),
            (Kind::Test, None) if test.label.starts_with("build") => {
                self.built.insert(run, None);
                Ok(Launch::BuildFirst(build(&test.id)))
            }
            (_, Some(data)) => {
                let mut base = fenix_extension_api::serde_json::Map::new();
                base.insert("data".into(), data.into());
                Ok(Launch::Launch(LaunchConfig::launch(&test.label, &["fake"]).program("/data/fake-debugger").arg(test.id).arg(format!("run {run}")).env("AT", "${data}").attributes(&attributes(base))))
            }
            _ => Err(format!("{} can't be debugged", test.id)),
        }
    }
}

export!(Provider);
