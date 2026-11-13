// What Fenix 1.5 links: every import of 1.5's `host`, as wasmtime 50
// binds `wit-1.5/world.wit` (v1.5.2's, unchanged). Only `log` does
// anything; the guests these tests start never call the rest, and fail
// the test if they do. Written out from the WIT function by function, so
// a difference from what 1.5's bindings want is a compile error.
impl v15::fenix::extension::host::Host for Host15 {
    fn log(&mut self, _level: T::Level, message: String) {
        self.said.push(message)
    }
    fn notify(&mut self, _level: T::Level, _message: String) {
        unreachable!("notify")
    }
    fn notify_with(&mut self, _notification: T::Notification) {
        unreachable!("notify-with")
    }
    fn notify_dismiss(&mut self, _tag: String) {
        unreachable!("notify-dismiss")
    }
    fn run_command(&mut self, _id: String) {
        unreachable!("run-command")
    }
    fn open_file(&mut self, _path: String, _line: Option<u32>) {
        unreachable!("open-file")
    }
    fn show_picker(&mut self, _title: String, _items: Vec<T::PickerItem>) -> u64 {
        unreachable!("show-picker")
    }
    fn show_picker_many(&mut self, _title: String, _items: Vec<T::PickerItem>) -> u64 {
        unreachable!("show-picker-many")
    }
    fn prompt(&mut self, _label: String, _initial: String) -> u64 {
        unreachable!("prompt")
    }
    fn compose(&mut self, _title: String, _verb: String, _initial: String, _allow_empty: bool) -> u64 {
        unreachable!("compose")
    }
    fn compose_with_ask(&mut self, _title: String, _verb: String, _initial: String, _allow_empty: bool, _ask: String, _over_initial: bool) -> u64 {
        unreachable!("compose-with-ask")
    }
    fn editor_here(&mut self) -> Option<T::Here> {
        unreachable!("editor-here")
    }
    fn home_set(&mut self, _section: Option<T::HomeSection>) {
        unreachable!("home-set")
    }
    fn page_open(&mut self, _kind: String, _title: String) -> u64 {
        unreachable!("page-open")
    }
    fn page_open_beside(&mut self, _kind: String, _title: String, _width: f32) -> u64 {
        unreachable!("page-open-beside")
    }
    fn page_refresh(&mut self, _page: u64) {
        unreachable!("page-refresh")
    }
    fn page_close(&mut self, _page: u64) {
        unreachable!("page-close")
    }
    fn open_text(&mut self, _title: String, _text: String, _language: Option<String>) {
        unreachable!("open-text")
    }
    fn open_url(&mut self, _url: String) -> Result<(), String> {
        unreachable!("open-url")
    }
    fn clipboard_set(&mut self, _text: String) {
        unreachable!("clipboard-set")
    }
    fn clipboard_set_image(&mut self, _png: Vec<u8>) -> Result<(), String> {
        unreachable!("clipboard-set-image")
    }
    fn clipboard_image(&mut self) -> Option<Vec<u8>> {
        unreachable!("clipboard-image")
    }
    fn buffer_text(&mut self, _path: String) -> Option<String> {
        unreachable!("buffer-text")
    }
    fn buffer_edit(&mut self, _path: String, _edits: Vec<T::TextEdit>) -> Result<(), String> {
        unreachable!("buffer-edit")
    }
    fn buffer_insert(&mut self, _text: String) -> Result<(), String> {
        unreachable!("buffer-insert")
    }
    fn buffer_save(&mut self, _path: String) -> Result<(), String> {
        unreachable!("buffer-save")
    }
    fn status_set(&mut self, _text: Option<String>) {
        unreachable!("status-set")
    }
    fn setting(&mut self, _key: String) -> Option<String> {
        unreachable!("setting")
    }
    fn theme_colors(&mut self) -> T::EditorTheme {
        unreachable!("theme-colors")
    }
    fn editor_keys(&mut self) -> T::KeyScheme {
        unreachable!("editor-keys")
    }
    fn key_for(&mut self, _command: String) -> Option<String> {
        unreachable!("key-for")
    }
    fn setting_set(&mut self, _key: String, _value: Option<String>) -> Result<(), String> {
        unreachable!("setting-set")
    }
    fn secret(&mut self, _key: String) -> Option<String> {
        unreachable!("secret")
    }
    fn find_program(&mut self, _name: String, _hints: Vec<String>) -> Option<String> {
        unreachable!("find-program")
    }
    fn new_project(&mut self, _args: String) {
        unreachable!("new-project")
    }
    fn spawn(&mut self, _cmd: T::Command) -> Result<u64, String> {
        unreachable!("spawn")
    }
    fn write_stdin(&mut self, _process: u64, _data: Vec<u8>) -> Result<(), String> {
        unreachable!("write-stdin")
    }
    fn kill(&mut self, _process: u64) {
        unreachable!("kill")
    }
    fn run_in_terminal(&mut self, _cmd: T::Command, _title: String, _below: bool) -> Result<u64, String> {
        unreachable!("run-in-terminal")
    }
    fn terminal_stop(&mut self, _terminal: u64) {
        unreachable!("terminal-stop")
    }
    fn terminal_restart(&mut self, _terminal: u64, _cmd: Option<T::Command>, _title: Option<String>) -> Result<(), String> {
        unreachable!("terminal-restart")
    }
    fn run_task(&mut self, _name: String, _cmd: T::Command) -> Result<u64, String> {
        unreachable!("run-task")
    }
    fn utc_offset(&mut self, _at: i64) -> i32 {
        unreachable!("utc-offset")
    }
    fn set_timer(&mut self, _millis: u32, _repeat: bool) -> u64 {
        unreachable!("set-timer")
    }
    fn git_state(&mut self) -> Option<T::GitSummary> {
        unreachable!("git-state")
    }
    fn last_task(&mut self) -> Option<T::TaskResult> {
        unreachable!("last-task")
    }
    fn cancel_timer(&mut self, _timer: u64) {
        unreachable!("cancel-timer")
    }
    fn call_service(&mut self, _name: String, _payload: String) -> u64 {
        unreachable!("call-service")
    }
    fn service_answer(&mut self, _call: u64, _answer: Result<String, String>) {
        unreachable!("service-answer")
    }
    fn service_progress(&mut self, _call: u64, _message: String, _fraction: Option<f32>) {
        unreachable!("service-progress")
    }
    fn image_load(&mut self, _bytes: Vec<u8>) -> Result<T::ImageInfo, String> {
        unreachable!("image-load")
    }
    fn image_file(&mut self, _path: String) -> Result<T::ImageInfo, String> {
        unreachable!("image-file")
    }
    fn image_free(&mut self, _image: u32) {
        unreachable!("image-free")
    }
    fn lsp_send(&mut self, _server: u64, _message: String) {
        unreachable!("lsp-send")
    }
    fn language_server_set(&mut self, _languages: Vec<String>, _cmd: Option<T::Command>) -> Result<(), String> {
        unreachable!("language-server-set")
    }
    fn language_server_restart(&mut self) {
        unreachable!("language-server-restart")
    }
    fn doctor_set(&mut self, _checks: Vec<T::Check>) {
        unreachable!("doctor-set")
    }
    fn tasks_set(&mut self, _tasks: Vec<T::Task>) -> Result<(), String> {
        unreachable!("tasks-set")
    }
    fn sheet_load(&mut self, _path: String, _frame_width: u32, _frame_height: u32) -> Result<u32, String> {
        unreachable!("sheet-load")
    }
    fn model_load(&mut self, _path: String) -> Result<T::ModelInfo, String> {
        unreachable!("model-load")
    }
    fn layer_open(&mut self, _level: T::LayerLevel) -> Result<u64, String> {
        unreachable!("layer-open")
    }
    fn layer_set(&mut self, _layer: u64, _items: Vec<T::Item>) {
        unreachable!("layer-set")
    }
    fn layer_close(&mut self, _layer: u64) {
        unreachable!("layer-close")
    }
    fn socket_connect(&mut self, _host: String, _port: u16) -> Result<u64, String> {
        unreachable!("socket-connect")
    }
    fn socket_write(&mut self, _socket: u64, _data: Vec<u8>) -> Result<(), String> {
        unreachable!("socket-write")
    }
    fn udp_bind(&mut self, _port: u16) -> Result<u64, String> {
        unreachable!("udp-bind")
    }
    fn udp_send(&mut self, _socket: u64, _host: String, _port: u16, _data: Vec<u8>) -> Result<(), String> {
        unreachable!("udp-send")
    }
    fn socket_close(&mut self, _socket: u64) {
        unreachable!("socket-close")
    }
    fn surface_open(&mut self, _kind: String, _title: String, _options: T::SurfaceOptions) -> Result<u64, String> {
        unreachable!("surface-open")
    }
    fn surface_resize(&mut self, _surface: u64, _width: u32, _height: u32) -> Result<(), String> {
        unreachable!("surface-resize")
    }
    fn surface_write(&mut self, _surface: u64, _x: u32, _y: u32, _width: u32, _height: u32, _bgra: Vec<u8>) -> Result<(), String> {
        unreachable!("surface-write")
    }
    fn surface_fill(&mut self, _surface: u64, _x: u32, _y: u32, _width: u32, _height: u32, _color: T::Rgb) -> Result<(), String> {
        unreachable!("surface-fill")
    }
    fn surface_copy(&mut self, _surface: u64, _x: u32, _y: u32, _width: u32, _height: u32, _to_x: u32, _to_y: u32) -> Result<(), String> {
        unreachable!("surface-copy")
    }
    fn surface_present(&mut self, _surface: u64) {
        unreachable!("surface-present")
    }
    fn surface_cursor(&mut self, _surface: u64, _cursor: Option<T::CursorImage>) {
        unreachable!("surface-cursor")
    }
    fn surface_marks(&mut self, _surface: u64, _marks: Vec<T::SurfaceMark>) {
        unreachable!("surface-marks")
    }
    fn page_retitle(&mut self, _page: u64, _title: String) {
        unreachable!("page-retitle")
    }
    fn spawn_raw(&mut self, _cmd: T::Command) -> Result<u64, String> {
        unreachable!("spawn-raw")
    }
    fn clipboard_text(&mut self) -> Option<String> {
        unreachable!("clipboard-text")
    }
    fn file_read(&mut self, _path: String) -> Result<Vec<u8>, String> {
        unreachable!("file-read")
    }
    fn explorer_selected(&mut self) -> Option<String> {
        unreachable!("explorer-selected")
    }
    fn git_refresh(&mut self) {
        unreachable!("git-refresh")
    }
    fn page_focus(&mut self, _page: u64) {
        unreachable!("page-focus")
    }
    fn editor_cursor(&mut self) -> Option<T::CursorInfo> {
        unreachable!("editor-cursor")
    }
    fn scratch_open(&mut self, _title: String, _text: String, _language: Option<String>) -> u64 {
        unreachable!("scratch-open")
    }
    fn scratch_text(&mut self, _scratch: u64) -> Option<String> {
        unreachable!("scratch-text")
    }
    fn scratch_set(&mut self, _scratch: u64, _text: String) -> Result<(), String> {
        unreachable!("scratch-set")
    }
    fn scratch_close(&mut self, _scratch: u64) {
        unreachable!("scratch-close")
    }
    fn folder_list(&mut self, _path: String) -> Result<Vec<T::FolderEntry>, String> {
        unreachable!("folder-list")
    }
    fn play_sound(&mut self, _path: String) -> Result<(), String> {
        unreachable!("play-sound")
    }
    fn decorations_set(&mut self, _buffer: T::TextBuffer, _decorations: Vec<T::Decoration>) -> Result<(), String> {
        unreachable!("decorations-set")
    }
    fn buffer_undo(&mut self, _path: String) -> Result<(), String> {
        unreachable!("buffer-undo")
    }
    fn buffer_redo(&mut self, _path: String) -> Result<(), String> {
        unreachable!("buffer-redo")
    }
    fn theme_palette(&mut self) -> Vec<T::Rgb> {
        unreachable!("theme-palette")
    }
    fn debug_adapter_set(&mut self, _languages: Vec<String>, _cmd: Option<T::Command>) -> Result<(), String> {
        unreachable!("debug-adapter-set")
    }
    fn formatter_set(&mut self, _languages: Vec<String>, _cmd: Option<T::Command>, _keep_output: bool) -> Result<(), String> {
        unreachable!("formatter-set")
    }
    fn diagnostics_set(&mut self, _path: String, _problems: Vec<T::Problem>) -> Result<(), String> {
        unreachable!("diagnostics-set")
    }
    fn close_stdin(&mut self, _process: u64) {
        unreachable!("close-stdin")
    }
    fn scratch_open_beside(&mut self, _title: String, _text: String, _language: Option<String>, _width: f32) -> u64 {
        unreachable!("scratch-open-beside")
    }
    fn open_text_beside(&mut self, _title: String, _text: String, _language: Option<String>, _width: f32) {
        unreachable!("open-text-beside")
    }
    fn http_download(&mut self, _req: T::HttpRequest, _path: String, _sha256: Option<String>) -> Result<u64, String> {
        unreachable!("http-download")
    }
    fn set_executable(&mut self, _path: String) -> Result<(), String> {
        unreachable!("set-executable")
    }
    fn http(&mut self, _req: T::HttpRequest) -> Result<u64, String> {
        unreachable!("http")
    }
    fn http_cancel(&mut self, _request: u64) {
        unreachable!("http-cancel")
    }
    fn socket_connect_tls(&mut self, _host: String, _port: u16, _tls: T::TlsOptions) -> Result<u64, String> {
        unreachable!("socket-connect-tls")
    }
    fn socket_start_tls(&mut self, _socket: u64, _tls: T::TlsOptions) -> Result<(), String> {
        unreachable!("socket-start-tls")
    }
    fn ipc_connect(&mut self, _name: String) -> Result<u64, String> {
        unreachable!("ipc-connect")
    }
    fn debug_adapter_connect(&mut self, _languages: Vec<String>, _cmd: Option<T::Command>, _port: u16) -> Result<(), String> {
        unreachable!("debug-adapter-connect")
    }
    fn decoration_rules_set(&mut self, _rules: Vec<T::DecorationRule>) -> Result<(), String> {
        unreachable!("decoration-rules-set")
    }
    fn page_file(&mut self, _page: u64, _path: Option<String>) -> Result<(), String> {
        unreachable!("page-file")
    }
    fn platform(&mut self) -> T::PlatformInfo {
        unreachable!("platform")
    }
    fn buffer_language_set(&mut self, _buffer: T::TextBuffer, _language: Option<String>) -> Result<(), String> {
        unreachable!("buffer-language-set")
    }
    fn run_task_background(&mut self, _name: String, _cmd: T::Command) -> Result<u64, String> {
        unreachable!("run-task-background")
    }
    fn task_cancel(&mut self, _task: u64) {
        unreachable!("task-cancel")
    }
    fn task_show(&mut self, _task: u64) -> Result<(), String> {
        unreachable!("task-show")
    }
    fn language_server_add(&mut self, _name: String, _languages: Vec<String>, _cmd: Option<T::Command>) -> Result<(), String> {
        unreachable!("language-server-add")
    }
    fn process_id(&mut self) -> u32 {
        unreachable!("process-id")
    }
    fn language_server_settings(&mut self, _name: String, _settings: Option<String>) -> Result<(), String> {
        unreachable!("language-server-settings")
    }
    fn launches_set(&mut self, _launches: Vec<T::LaunchConfig>) -> Result<(), String> {
        unreachable!("launches-set")
    }
    fn launch_variable_set(&mut self, _name: String, _choices: Vec<T::PickerItem>) -> Result<(), String> {
        unreachable!("launch-variable-set")
    }
    fn secret_set(&mut self, _key: String, _value: Option<String>) -> Result<(), String> {
        unreachable!("secret-set")
    }
    fn prompt_secret(&mut self, _label: String) -> u64 {
        unreachable!("prompt-secret")
    }
    fn tcp_listen(&mut self, _port: u16) -> Result<(u64, u16), String> {
        unreachable!("tcp-listen")
    }
    fn env_var(&mut self, _name: String) -> Option<String> {
        unreachable!("env-var")
    }
    fn agent_provide(&mut self, _id: String, _cmd: Option<T::Command>) -> Result<(), String> {
        unreachable!("agent-provide")
    }
}
