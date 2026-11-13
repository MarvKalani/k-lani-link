//! Model switches against synthetic engines served in this process; no model, GPU or
//! interpreter needed. One test drives real child processes to check the group stop.
use super::*;
use crate::supervisor::catalog;
use std::sync::Mutex as Plain;
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    root: PathBuf,
    doc: Value,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new() -> Self {
        // Short: macOS keeps socket paths under 104 bytes, and its temp folder is long.
        let root =
            std::env::temp_dir().join(format!("ls-{}", &crate::config::random_id().unwrap()[..12]));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        for file in ["pack.basalt", "model.gguf"] {
            std::fs::write(root.join(file), b"synthetic test fixture").unwrap();
        }
        let pack = root.join("pack.basalt").display().to_string();
        let mut models: Vec<Value> = ["alpha", "beta", "unsafe"]
            .iter()
            .map(|id| json!({"id": id, "path": pack, "parallel": 2, "context_tokens": 262144, "slot_full_context": true}))
            .collect();
        models.push(json!({"id": "Mellum2.1-Q8_0", "engine": "llama",
            "path": root.join("model.gguf").display().to_string(),
            "parallel": 2, "context_tokens": 32768, "kv": "q4_0"}));
        Self {
            doc: json!({"default": "alpha", "models": models}),
            root,
        }
    }
    fn catalog(&self) -> Result<Catalog> {
        catalog::from_slice(self.doc.to_string().as_bytes())
    }
    fn refused(&self, edit: impl FnOnce(&mut Value)) -> bool {
        let mut doc = self.doc.clone();
        edit(&mut doc);
        catalog::from_slice(doc.to_string().as_bytes()).is_err()
    }
    fn settings(&self) -> Settings {
        Settings {
            runtime: self.root.clone(),
            basalt: vec!["fake-basalt".into()],
            basalt_answer_room_value: true,
            llama: vec![self.root.join("llama-server").display().to_string()],
            metrics: false,
            timeout: Duration::from_millis(800),
            require_no_swap: false,
            swap_disabled: || true,
        }
    }
    fn manager(&self, fake: &Fake) -> Manager {
        Manager::new(
            self.catalog().unwrap(),
            self.settings(),
            Box::new(fake.clone()),
        )
    }
}

#[derive(Clone)]
struct Spawned {
    args: Vec<String>,
    alive: Arc<AtomicBool>,
}
#[derive(Clone, Default)]
struct Fake(Arc<Plain<Vec<Spawned>>>);
impl Fake {
    fn last(&self) -> Spawned {
        self.0.lock().unwrap().last().cloned().unwrap()
    }
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}
fn value_of(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}
fn number_of(args: &[String], flag: &str) -> i64 {
    value_of(args, flag)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
struct FakeProcess {
    task: Option<tokio::task::JoinHandle<()>>,
    alive: Arc<AtomicBool>,
    exit: Option<i32>,
}
impl Process for FakeProcess {
    fn running(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.exit
    }
    fn stop(&mut self, _: Duration) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.alive.store(false, Ordering::SeqCst);
    }
}
impl Launcher for Fake {
    fn spawn(
        &self,
        args: &[String],
        env: Option<Vec<(String, String)>>,
    ) -> Result<Box<dyn Process>> {
        let alive = Arc::new(AtomicBool::new(true));
        self.0.lock().unwrap().push(Spawned {
            args: args.to_vec(),
            alive: alive.clone(),
        });
        let alias = value_of(args, "--alias").unwrap_or_default();
        let parallel = number_of(args, "--parallel");
        let llama = args[0].ends_with("llama-server");
        let (socket, routes) = if llama {
            // The supervisor must switch off everything that could keep or show a request.
            let quiet = [
                "--no-webui",
                "--no-slots",
                "--log-disable",
                "--offline",
                "--kv-unified",
            ]
            .iter()
            .all(|flag| args.iter().any(|a| a == flag));
            let libraries = env
                .unwrap_or_default()
                .iter()
                .any(|(key, value)| key == "LD_LIBRARY_PATH" && value.contains("/lib:"));
            if !quiet || !libraries {
                alive.store(false, Ordering::SeqCst);
                return Ok(Box::new(FakeProcess {
                    task: None,
                    alive,
                    exit: Some(3),
                }));
            }
            let context = number_of(args, "--ctx-size");
            (
                value_of(args, "--host").unwrap(),
                vec![
                    ("/health", json!({"status": "ok"})),
                    (
                        "/props",
                        json!({"total_slots": parallel, "default_generation_settings": {"n_ctx": context}}),
                    ),
                    ("/v1/models", json!({"data": [{"id": alias}]})),
                ],
            )
        } else {
            let full = args.iter().any(|a| a == "--slot-full-context");
            let context = number_of(args, "--max-context");
            let unsafe_log = alias == "unsafe";
            (
                value_of(args, "--unix-socket").unwrap(),
                vec![(
                    "/health",
                    json!({"loaded": true, "model": alias, "parallel": parallel,
                        "max_context": if full { context } else { context / parallel },
                        "link_privacy": {"version": 1, "application_logs": unsafe_log,
                            "request_history": false, "diagnostics": false, "remote_images": false}}),
                )],
            )
        };
        let mut app = Router::new();
        for (route, body) in routes {
            app = app.route(route, get(move || async move { axum::Json(body) }));
        }
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Box::new(FakeProcess {
            task: Some(task),
            alive,
            exit: None,
        }))
    }
}

async fn load(manager: &mut Manager, model: &str) -> (StatusCode, Value) {
    manager
        .command(&json!({"action": "load", "model": model}))
        .await
}

#[tokio::test]
async fn load_switch_unload_without_arbitrary_paths() {
    let fixture = Fixture::new();
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    let (code, state) = load(&mut manager, "alpha").await;
    assert_eq!(
        (code, &state["active_model"]),
        (StatusCode::OK, &json!("alpha"))
    );
    let first = fake.last();
    assert_eq!(
        load(&mut manager, "../../secret").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(load(&mut manager, "missing").await.0, StatusCode::NOT_FOUND);
    let with_path = json!({"action": "load", "model": "beta", "path": "/etc/passwd"});
    assert_eq!(manager.command(&with_path).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(fake.count(), 1);
    assert!(first.alive.load(Ordering::SeqCst));
    assert_eq!(load(&mut manager, "beta").await.0, StatusCode::OK);
    assert!(!first.alive.load(Ordering::SeqCst));
    assert_eq!(
        basalt_ready(&manager.socket()).await.unwrap()["model"],
        "beta"
    );
    // The same model again is a no-op, not a restart.
    assert_eq!(load(&mut manager, "beta").await.0, StatusCode::OK);
    assert_eq!(fake.count(), 2);
    let unload = manager.command(&json!({"action": "unload"})).await;
    assert_eq!(unload.0, StatusCode::OK);
    assert!(!manager.socket().exists());
    let snapshot = manager.snapshot().await;
    assert!(snapshot["active_model"].is_null());
    assert!(!snapshot.to_string().contains("path"));
    for invalid in [
        json!({"action": "unload", "model": "alpha"}),
        json!(["unload"]),
        json!({"action": "load"}),
    ] {
        assert_eq!(manager.command(&invalid).await.0, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn a_gguf_model_runs_under_llama_cpp_and_is_vouched_for() {
    let fixture = Fixture::new();
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    assert_eq!(load(&mut manager, "alpha").await.0, StatusCode::OK);
    let (code, state) = load(&mut manager, "Mellum2.1-Q8_0").await;
    assert_eq!(code, StatusCode::OK, "{state}");
    assert_eq!(state["active_model"], "Mellum2.1-Q8_0");
    let rows = state["models"].as_array().unwrap();
    assert_eq!(rows[3]["privacy"], "supervised");
    assert_eq!(rows[3]["engine"], "llama");
    assert_eq!(rows[3]["context_per_slot"], 32768);
    assert!(rows[0]["privacy"].is_null());
    let args = fake.last().args;
    assert_eq!(value_of(&args, "--cache-type-k").unwrap(), "q4_0");
    assert_eq!(value_of(&args, "--reasoning-budget").unwrap(), "2048");
    assert_eq!(load(&mut manager, "beta").await.0, StatusCode::OK);
    assert_eq!(manager.snapshot().await["models"][1]["privacy"], "basalt");
}

#[tokio::test]
async fn backend_without_privacy_is_stopped_and_not_reported_as_loaded() {
    let fixture = Fixture::new();
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    assert_eq!(
        load(&mut manager, "unsafe").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(manager.process.is_none());
    assert!(!fake.last().alive.load(Ordering::SeqCst));
    assert!(manager.snapshot().await["active_model"].is_null());
    assert!(!manager.socket().exists());
}

#[tokio::test]
async fn a_context_split_across_slots_is_checked_per_slot() {
    let fixture = Fixture::new();
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    // The fake reports what it was started with; a profile that splits the context
    // per slot must see exactly that split back.
    manager.catalog.models[0].slot_full_context = false;
    assert_eq!(load(&mut manager, "alpha").await.0, StatusCode::OK);
    assert_eq!(
        manager.snapshot().await["models"][0]["context_per_slot"],
        131072
    );
}

#[tokio::test]
async fn llama_can_use_its_unrestricted_reasoning_budget() {
    let mut fixture = Fixture::new();
    fixture.doc["models"][3]["reasoning_budget_tokens"] = json!(-1);
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    assert_eq!(load(&mut manager, "Mellum2.1-Q8_0").await.0, StatusCode::OK);
    assert_eq!(
        value_of(&fake.last().args, "--reasoning-budget").unwrap(),
        "-1"
    );
    assert!(fixture.refused(|doc| doc["models"][0]["reasoning_budget_tokens"] = json!(-1)));
}

#[tokio::test]
async fn operator_expert_profile_reaches_basalt_without_exposing_its_path() {
    let mut fixture = Fixture::new();
    let profile = fixture.root.join("custom.prof");
    std::fs::write(&profile, b"operator-selected fixture").unwrap();
    let profile = profile.display().to_string();
    fixture.doc["models"][0]["expert_profile"] = json!(profile);
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    let (code, state) = load(&mut manager, "alpha").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        value_of(&fake.last().args, "--expert-profile").unwrap(),
        profile
    );
    assert!(!state.to_string().contains(&profile));
    let other = json!({"action": "load", "model": "alpha", "expert_profile": "/tmp/other.prof"});
    assert_eq!(manager.command(&other).await.0, StatusCode::BAD_REQUEST);
}

#[test]
fn expert_profiles_require_existing_absolute_files_and_basalt() {
    let fixture = Fixture::new();
    let root = fixture.root.display().to_string();
    for value in [
        json!(null),
        json!(123),
        json!("relative.prof"),
        json!(format!("{root}/missing.prof")),
        json!(root),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][0]["expert_profile"] = value.clone()),
            "{value}"
        );
    }
    let profile = fixture.root.join("custom.prof");
    std::fs::write(&profile, b"fixture").unwrap();
    let profile = profile.display().to_string();
    assert!(fixture.refused(|doc| doc["models"][3]["expert_profile"] = json!(profile)));
}

#[tokio::test]
async fn operator_performance_profile_reaches_the_backend_but_not_the_remote_api() {
    let mut fixture = Fixture::new();
    for (key, value) in [
        ("memory_mode", json!("resident")),
        ("prefill", json!("auto:16384")),
        ("prefill_cpu_share", json!(0.4)),
        ("pool_workers", json!(3)),
    ] {
        fixture.doc["models"][0][key] = value;
    }
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    let (code, state) = load(&mut manager, "alpha").await;
    assert_eq!(code, StatusCode::OK);
    let args = fake.last().args;
    assert!(args.iter().any(|a| a == "--resident-experts"));
    assert!(!args.iter().any(|a| a == "--mmap-experts"));
    for (flag, value) in [
        ("--prefill", "auto:16384"),
        ("--prefill-cpu-share", "0.4"),
        ("--pool-workers", "3"),
    ] {
        assert_eq!(value_of(&args, flag).unwrap(), value);
    }
    assert!(!state.to_string().contains("memory_mode"));
    let remote = json!({"action": "load", "model": "alpha", "memory_mode": "mmap"});
    assert_eq!(manager.command(&remote).await.0, StatusCode::BAD_REQUEST);
}

#[test]
fn invalid_performance_settings_are_rejected_before_a_process_starts() {
    let fixture = Fixture::new();
    for (key, value) in [
        ("memory_mode", json!("resident --debug")),
        ("memory_mode", json!(null)),
        ("prefill", json!(true)),
        ("prefill", json!("auto:9999")),
        ("prefill", json!("auto --debug")),
        ("prefill", json!(64)),
        ("prefill", json!(512.0)),
        ("prefill_cpu_share", json!(true)),
        ("prefill_cpu_share", json!(1.1)),
        ("prefill_cpu_share", json!(-0.1)),
        ("pool_workers", json!(true)),
        ("pool_workers", json!(0)),
        ("pool_workers", json!(257)),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][0][key] = value.clone()),
            "{key} {value}"
        );
    }
    for (key, value) in [
        ("memory_mode", json!("resident")),
        ("prefill_cpu_share", json!(0.4)),
        ("pool_workers", json!(3)),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][3][key] = value.clone()),
            "{key}"
        );
    }
    // JSON itself has no NaN or infinity; such a file does not parse at all.
    let text = fixture.doc.to_string().replacen(
        "\"parallel\":2",
        "\"prefill_cpu_share\":NaN,\"parallel\":2",
        1,
    );
    assert!(catalog::from_slice(text.as_bytes()).is_err());
}

static SWAP_OFF: AtomicBool = AtomicBool::new(true);

#[tokio::test]
async fn swap_guard_rechecks_before_switching_and_keeps_the_loaded_model() {
    let fixture = Fixture::new();
    let fake = Fake::default();
    let mut settings = fixture.settings();
    settings.require_no_swap = true;
    settings.swap_disabled = || SWAP_OFF.load(Ordering::SeqCst);
    let mut manager = Manager::new(fixture.catalog().unwrap(), settings, Box::new(fake.clone()));
    assert_eq!(load(&mut manager, "alpha").await.0, StatusCode::OK);
    let first = fake.last();
    SWAP_OFF.store(false, Ordering::SeqCst);
    let (code, body) = load(&mut manager, "beta").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"].as_str().unwrap().contains("swap"));
    assert!(first.alive.load(Ordering::SeqCst));
    assert_eq!(
        load(&mut manager, "alpha").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    SWAP_OFF.store(true, Ordering::SeqCst);
    assert_eq!(load(&mut manager, "beta").await.0, StatusCode::OK);
    assert!(!first.alive.load(Ordering::SeqCst));
}

#[tokio::test]
async fn qwen_sampling_and_context_fitting_reach_basalt_without_remote_policy_changes() {
    let mut fixture = Fixture::new();
    let sampling = json!({"temperature": 1.0, "top_p": 0.95, "top_k": 20, "min_p": 0.0,
        "presence_penalty": 0.0, "repetition_penalty": 1.0});
    for (key, value) in [
        ("reasoning_budget_tokens", json!(0)),
        ("answer_room", json!(false)),
        ("fit_max_tokens", json!(true)),
        ("sampling", sampling.clone()),
    ] {
        fixture.doc["models"][0][key] = value;
    }
    let fake = Fake::default();
    let mut manager = fixture.manager(&fake);
    let (code, state) = load(&mut manager, "alpha").await;
    assert_eq!(code, StatusCode::OK);
    let args = fake.last().args;
    assert_eq!(value_of(&args, "--reasoning-budget-tokens").unwrap(), "0");
    assert_eq!(value_of(&args, "--answer-room").unwrap(), "0");
    assert!(args.iter().any(|a| a == "--fit-max-tokens"));
    for (flag, value) in [
        ("--temperature", "1.0"),
        ("--top-p", "0.95"),
        ("--top-k", "20"),
        ("--min-p", "0.0"),
        ("--presence-penalty", "0.0"),
        ("--repetition-penalty", "1.0"),
    ] {
        assert_eq!(value_of(&args, flag).unwrap(), value, "{flag}");
    }
    assert!(!state.to_string().contains("sampling"));
    let remote = json!({"action": "load", "model": "alpha", "sampling": sampling});
    assert_eq!(manager.command(&remote).await.0, StatusCode::BAD_REQUEST);
    manager.stop_engine();
    manager.catalog.models[0].fit_max_tokens = false;
    assert_eq!(load(&mut manager, "alpha").await.0, StatusCode::OK);
    let args = fake.last().args;
    assert!(!args.iter().any(|a| a.contains("fit-max-tokens")));
    let mut legacy = fixture.settings();
    legacy.basalt_answer_room_value = false;
    assert!(
        launch_args(&legacy, &manager.catalog.models[0])
            .iter()
            .any(|arg| arg == "--no-answer-room")
    );
}

#[test]
fn sampling_settings_are_validated_and_not_applied_to_other_engines() {
    let fixture = Fixture::new();
    for value in [
        json!({"temperature": true}),
        json!({"top_p": 0}),
        json!({"top_p": 1.1}),
        json!({"top_k": 2.5}),
        json!({"top_k": -1}),
        json!({"min_p": -0.1}),
        json!({"repetition_penalty": 0}),
        json!({"before_load": "curl example.org"}),
        json!([]),
        json!(null),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][0]["sampling"] = value.clone()),
            "{value}"
        );
    }
    for (key, value) in [
        ("sampling", json!({"temperature": 1.0})),
        ("answer_room", json!(false)),
        ("fit_max_tokens", json!(true)),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][3][key] = value.clone()),
            "{key}"
        );
    }
    for key in ["answer_room", "fit_max_tokens"] {
        assert!(
            fixture.refused(|doc| doc["models"][0][key] = json!("false")),
            "{key}"
        );
    }
}

#[test]
fn catalog_rejects_unknown_fields_and_invalid_profiles() {
    let fixture = Fixture::new();
    assert!(fixture.catalog().is_ok());
    for (field, value) in [
        ("parallel", json!(9)),
        ("context_tokens", json!(true)),
        ("context_tokens", json!(4096.0)),
        ("slot_full_context", json!("yes")),
        ("before_load", json!("curl example.org")),
        ("engine", json!("vllm")),
        ("kv", json!("q4_0")),
        ("id", json!("../x")),
        ("id", json!(".hidden")),
        ("path", json!("relative.basalt")),
    ] {
        assert!(
            fixture.refused(|doc| doc["models"][0][field] = value.clone()),
            "{field}"
        );
    }
    assert!(fixture.refused(|doc| doc["models"][1]["id"] = json!("alpha")));
    assert!(fixture.refused(|doc| doc["default"] = json!("missing")));
    assert!(fixture.refused(|doc| doc["models"][3]["slot_full_context"] = json!(false)));
    assert!(fixture.refused(|doc| doc["models"][3]["kv"] = json!("int8")));
    assert!(fixture.refused(|doc| doc["extra"] = json!(1)));
}

#[tokio::test]
async fn the_control_socket_takes_only_small_exact_commands() {
    use tower::ServiceExt;
    let fixture = Fixture::new();
    let fake = Fake::default();
    let app = router(Arc::new(Mutex::new(fixture.manager(&fake))));
    let send = |method: &str, route: &str, body: &str, chunked: bool| {
        let mut request = Request::builder().method(method).uri(route);
        if chunked {
            request = request.header(header::TRANSFER_ENCODING, "chunked");
        } else {
            request = request.header(header::CONTENT_LENGTH, body.len());
        }
        request.body(Body::from(body.to_owned())).unwrap()
    };
    let status = |response: Response| response.status();
    let unload = r#"{"action":"unload"}"#;
    assert_eq!(
        status(
            app.clone()
                .oneshot(send("POST", "/v1/link/model", unload, false))
                .await
                .unwrap()
        ),
        StatusCode::OK
    );
    assert_eq!(
        status(
            app.clone()
                .oneshot(send("POST", "/v1/link/model", unload, true))
                .await
                .unwrap()
        ),
        StatusCode::BAD_REQUEST
    );
    let long = format!(
        r#"{{"action":"unload","x":"{}"}}"#,
        "a".repeat(MAX_COMMAND_BYTES)
    );
    assert_eq!(
        status(
            app.clone()
                .oneshot(send("POST", "/v1/link/model", &long, false))
                .await
                .unwrap()
        ),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(
            app.clone()
                .oneshot(send("POST", "/v1/link/model", "not json", false))
                .await
                .unwrap()
        ),
        StatusCode::BAD_REQUEST
    );
    for (method, route) in [
        ("GET", "/v1/link/model"),
        ("POST", "/v1/link/models"),
        ("GET", "/health"),
        ("GET", "/"),
    ] {
        let response = app
            .clone()
            .oneshot(send(method, route, "", false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {route}");
    }
    let response = app
        .oneshot(send("GET", "/v1/link/models", "", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn a_real_engine_and_everything_it_started_is_stopped() {
    let fixture = Fixture::new();
    let pid_file = fixture.root.join("child.pid");
    let mut settings = fixture.settings();
    settings.timeout = Duration::from_millis(400);
    // A shell that leaves a second process in its group and never becomes ready.
    settings.basalt = vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("sleep 30 & echo $! > {}; wait", pid_file.display()),
        "sh".into(),
    ];
    let mut manager = Manager::new(fixture.catalog().unwrap(), settings, Box::new(Processes));
    let (code, body) = load(&mut manager, "alpha").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["exit_code"].is_null(), "{body}");
    let child: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut gone = false;
    for _ in 0..50 {
        // The orphaned sleep is reaped by init; until then it may linger as a zombie.
        let status = std::fs::read_to_string(format!("/proc/{child}/stat")).unwrap_or_default();
        if status.is_empty() || status.contains(") Z ") {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "the engine's own child survived the stop");

    let mut settings = fixture.settings();
    settings.basalt = vec!["/bin/sh".into(), "-c".into(), "exit 7".into(), "sh".into()];
    let mut manager = Manager::new(fixture.catalog().unwrap(), settings, Box::new(Processes));
    let (code, body) = load(&mut manager, "alpha").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["exit_code"], 7);
}
