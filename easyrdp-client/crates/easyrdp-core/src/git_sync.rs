//! F6-V2（R-20261118-015）Git 同步核心逻辑（纯逻辑层，git 命令经 [`GitRunner`]
//! trait 抽象，单测用 mock 注入；真实 git 调用见 src-tauri/src/git_sync.rs）。
//!
//! 同步语义（MVP 口径）：
//! - 数据目录（app_data_dir）下 `sync/` 子目录作为独立 git 仓库；
//!   connections.json / groups.json / templates.json（存在才同步，F2-V2 扩展点）
//!   在每次变更时以**副本策略**原子写入 sync/；
//! - **安全边界**：副本落盘前，所有 `dpapi:<base64>` 凭据密文被替换为占位符
//!   `dpapi:<不可迁移>`（DPAPI 密文绑定本机用户，跨机器不可解，且泄漏密文熵）。
//!   pull 恢复时占位符字段置空（`""`），由用户重新填写凭据；
//! - commit message 固定格式：`sync: <变更摘要> @ <YYYY-MM-DD HH:MM:SS UTC>`；
//! - push：本地有变更才 commit，远端认证支持 HTTPS+token（URL 内嵌）或 SSH key；
//! - pull：远端提交时间 > 本地最后提交时间 = 远端较新：
//!   - 本地无未提交变更 → 直接覆盖本地（`reset --hard`）并提示恢复 N 条连接；
//!   - 本地也有变更（冲突）→ **本地优先**，远端版本另存 `<数据目录>/<文件名>.remote.bak`；
//!   - 远端较旧/相同 → 本地领先，仅 push；两边一致 → 无操作。

use serde_json::Value;
use std::path::{Path, PathBuf};

/// 同步仓库内参与同步的文件（相对数据目录 / sync/ 目录同名副本）。
/// templates.json 为 F2-V2 扩展点：数据目录不存在该文件时自动跳过。
pub const SYNC_FILES: [&str; 3] = ["connections.json", "groups.json", "templates.json"];

/// 凭据占位符（sync 副本中的 dpapi 密文统一替换成该值）。
pub const CRED_PLACEHOLDER: &str = "dpapi:<不可迁移>";
const CRED_PREFIX: &str = "dpapi:";

// ---------------------------------------------------------------------------
// 安全边界：密文 → 占位符 / 占位符 → 清空
// ---------------------------------------------------------------------------

/// 同步前净化：把 JSON 文本中所有 `dpapi:<base64>` 密文替换为 [`CRED_PLACEHOLDER`]。
///
/// 仅当 `dpapi:` 紧跟在 JSON 字符串起始引号 `"` 之后才认定是密文字段值
/// （避免误伤用户名等普通字符串里偶含的 "dpapi:" 字样）。密文体为 base64
/// 字符集（A-Za-z0-9+/=）连续段，整段替换为 `<不可迁移>`。
pub fn sanitize_for_sync(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < b.len() {
        // 候选起点：字符串开头（前一个字节是 `"`）且此处是 `dpapi:`
        let at_string_start = i >= 2 && b[i - 1] == b'"';
        if at_string_start && text[i..].starts_with(CRED_PREFIX) {
            let mut j = i + CRED_PREFIX.len();
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'+' || b[j] == b'/' || b[j] == b'=') {
                j += 1;
            }
            if j > i + CRED_PREFIX.len() {
                out.push_str("dpapi:<不可迁移>");
                i = j;
                continue;
            }
        }
        // 原样拷贝一个 char
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// pull 恢复：把副本中的占位符字段值清空（`"dpapi:<不可迁移>"` → `""`），
/// 由用户在 UI 中重新填写凭据。返回（净化后文本，清除的占位符个数）。
pub fn restore_from_sync(text: &str) -> (String, usize) {
    let mut count = 0usize;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(CRED_PLACEHOLDER) {
        // 占位符整段删除：字段值由 `"dpapi:<不可迁移>"` 变为空串 `""`
        out.push_str(&rest[..pos]);
        count += 1;
        rest = &rest[pos + CRED_PLACEHOLDER.len()..];
    }
    out.push_str(rest);
    (out, count)
}

/// 统计文本中的占位符个数（恢复前预估条数用）。
pub fn count_placeholders(text: &str) -> usize {
    text.matches(CRED_PLACEHOLDER).count()
}

// ---------------------------------------------------------------------------
// 变更摘要
// ---------------------------------------------------------------------------

/// 比较「JSON 数组」前后两版生成变更摘要（按 id 字段对齐，缺失回退 name）。
/// 返回 None 表示内容完全一致（无需 commit）。old 解析失败时按空表处理。
pub fn summarize_json_arrays(old: &str, new: &str, unit: &str) -> Option<String> {
    let old_arr: Vec<Value> = serde_json::from_str(old).unwrap_or_default();
    let new_arr: Vec<Value> =
        serde_json::from_str(new).map_err(|_| ()).unwrap_or_default();

    let key = |v: &Value| -> Option<String> {
        v.get("id").or_else(|| v.get("name")).and_then(|x| x.as_str()).map(str::to_string)
    };
    let canon = |v: &Value| serde_json::to_string(v).unwrap_or_default();

    let mut old_map = std::collections::HashMap::new();
    for v in &old_arr {
        if let Some(k) = key(v) {
            old_map.insert(k, canon(v));
        }
    }
    let mut added = 0usize;
    let mut changed = 0usize;
    let mut removed = 0usize;
    let mut new_keys = std::collections::HashSet::new();
    for v in &new_arr {
        let Some(k) = key(v) else { continue };
        new_keys.insert(k.clone());
        match old_map.get(&k) {
            None => added += 1,
            Some(prev) if *prev != canon(v) => changed += 1,
            _ => {}
        }
    }
    for k in old_map.keys() {
        if !new_keys.contains(k) {
            removed += 1;
        }
    }
    if added == 0 && changed == 0 && removed == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("新增 {added}"));
    }
    if changed > 0 {
        parts.push(format!("修改 {changed}"));
    }
    if removed > 0 {
        parts.push(format!("删除 {removed}"));
    }
    Some(format!("{}（{}）", parts.join("，"), unit))
}

// ---------------------------------------------------------------------------
// commit message 与时间戳
// ---------------------------------------------------------------------------

/// 固定格式：`sync: <变更摘要> @ <YYYY-MM-DD HH:MM:SS UTC>`。
pub fn build_commit_message(summary: &str, epoch_secs: i64) -> String {
    format!("sync: {} @ {} UTC", summary, format_utc_timestamp(epoch_secs))
}

/// epoch 秒 → `YYYY-MM-DD HH:MM:SS`（UTC，无外部依赖的 civil 算法）。
pub fn format_utc_timestamp(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs = epoch_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y, m, d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Howard Hinnant 的 days→civil 算法（公历，无外部依赖）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// pull 冲突策略（纯逻辑）
// ---------------------------------------------------------------------------

/// pull 决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullDecision {
    /// 远端无内容（仓库为空 / 分支不存在）。
    RemoteEmpty,
    /// 两边一致。
    UpToDate,
    /// 本地领先（远端较旧）：无需拉取，待 push。
    LocalAhead,
    /// 远端较新且本地干净：覆盖本地，恢复 `restored` 条连接。
    RemoteNewer { restored: usize },
    /// 远端较新且本地有变更（冲突）：本地优先，远端版本存 .bak。
    ConflictLocalWins,
}

/// pull 策略判定（纯逻辑）：
/// - `remote_newer`：远端最后提交时间 > 本地最后提交时间；
/// - `local_dirty`：sync 仓库有未提交变更；
/// - `remote_conn_count`：远端 connections.json 中的占位符计数近似。
pub fn decide_pull(remote_newer: bool, local_dirty: bool, remote_conn_count: usize) -> PullDecision {
    match (remote_newer, local_dirty) {
        (false, false) => PullDecision::UpToDate,
        (false, true) => PullDecision::LocalAhead,
        (true, false) => PullDecision::RemoteNewer { restored: remote_conn_count },
        (true, true) => PullDecision::ConflictLocalWins,
    }
}

// ---------------------------------------------------------------------------
// Git 命令抽象
// ---------------------------------------------------------------------------

/// 一次 git 命令的输出。
#[derive(Debug, Clone)]
pub struct GitOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl GitOut {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// git 命令执行抽象（单测 mock；生产实现见 [`RealGitRunner`]）。
pub trait GitRunner: Send + Sync {
    /// 在 `cwd` 目录执行 `git <args>`，注入 `envs`（如 GIT_SSH_COMMAND）。
    /// 进程启动失败（git 不存在等）返回 Err。
    fn run(&self, cwd: &Path, args: &[&str], envs: &[(String, String)]) -> Result<GitOut, String>;
}

/// 系统 git 实现（`git` 在 PATH；不引 git2 库，MVP 依赖最小化）。
pub struct RealGitRunner;

impl GitRunner for RealGitRunner {
    fn run(&self, cwd: &Path, args: &[&str], envs: &[(String, String)]) -> Result<GitOut, String> {
        // R-20261126-007：GUI 子系统程序 spawn 控制台子进程（git）时会分配新
        // 控制台 → 启动/同步时闪现黑色控制台窗口。Windows 下强制 CREATE_NO_WINDOW。
        let mut cmd = std::process::Command::new("git");
        cmd.args(args)
            .current_dir(cwd)
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = cmd
            .output()
            .map_err(|e| format!("执行 git 失败（请确认 git 已安装并在 PATH 中）: {e}"))?;
        Ok(GitOut {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        })
    }
}

/// 检测系统 git 是否可用（`git --version`）。
pub fn git_available() -> bool {
    // R-20261126-007：启动期探测同样不得闪现控制台窗口（CREATE_NO_WINDOW）。
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 认证
// ---------------------------------------------------------------------------

/// 同步仓库远端认证方式。
#[derive(Debug, Clone)]
pub enum SyncAuth {
    /// 无认证（本地仓库 / 免密远端）。
    None,
    /// HTTPS + token：URL 内嵌 `x-access-token:<token>@`。
    HttpsToken(String),
    /// SSH 私钥路径：经 GIT_SSH_COMMAND 注入。
    SshKey(PathBuf),
}

impl SyncAuth {
    /// 远端 URL（HTTPS token 内嵌；SSH URL 原样）。
    pub fn remote_url(&self, remote: &str) -> String {
        match self {
            SyncAuth::HttpsToken(token) if !token.is_empty() => {
                // https://host/... -> https://x-access-token:<token>@host/...
                if let Some(rest) = remote
                    .strip_prefix("https://")
                    .or_else(|| remote.strip_prefix("http://"))
                {
                    let scheme = if remote.starts_with("https://") { "https" } else { "http" };
                    // 剥离可能存在的旧 userinfo（user[:pass]@），保留主机名 + 路径
                    let (authority, path) = match rest.find('/') {
                        Some(slash) => (&rest[..slash], &rest[slash..]),
                        None => (rest, ""),
                    };
                    let authority = match authority.rfind('@') {
                        Some(at) => &authority[at + 1..],
                        None => authority,
                    };
                    return format!("{scheme}://x-access-token:{token}@{authority}{path}");
                }
                remote.to_string()
            }
            _ => remote.to_string(),
        }
    }

    /// 运行 git 命令所需的环境变量（SSH 私钥）。
    pub fn envs(&self) -> Vec<(String, String)> {
        match self {
            SyncAuth::SshKey(key) if !key.as_os_str().is_empty() => vec![(
                "GIT_SSH_COMMAND".into(),
                format!(
                    "ssh -i \"{}\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
                    key.display()
                ),
            )],
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// SyncManager：sync/ 仓库生命周期
// ---------------------------------------------------------------------------

/// sync 副本仓库管理器。所有 git 交互经 [`GitRunner`]，纯逻辑可 mock。
pub struct SyncManager<'a> {
    pub runner: &'a dyn GitRunner,
    /// 应用数据目录（connections.json 等）。
    pub data_dir: PathBuf,
    /// 同步仓库目录（数据目录下 sync/）。
    pub sync_dir: PathBuf,
    pub branch: String,
}

/// git 用户身份（仓库局部配置，不影响全局）。
const GIT_IDENTITY: [(&str, &str); 2] = [
    ("user.name", "EasyRDP Sync"),
    ("user.email", "sync@easyrdp.local"),
];

impl<'a> SyncManager<'a> {
    pub fn new(runner: &'a dyn GitRunner, data_dir: PathBuf, branch: &str) -> Self {
        Self {
            runner,
            sync_dir: data_dir.join("sync"),
            data_dir,
            branch: branch.to_string(),
        }
    }

    fn git(&self, args: &[&str], envs: &[(String, String)]) -> Result<GitOut, String> {
        self.runner.run(&self.sync_dir, args, envs)
    }

    /// 初始化 sync/ 仓库（幂等）：目录、git init -b <branch>、局部身份配置。
    pub fn ensure_repo(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.sync_dir)
            .map_err(|e| format!("创建 sync 目录失败: {e}"))?;
        let head = self.sync_dir.join(".git").join("HEAD");
        if !head.exists() {
            // git init -b <branch>（老版本 git 不支持 -b 时回退 init + checkout -B）
            let out = self.git(&["init", "-b", &self.branch], &[])?;
            if !out.ok() {
                let out = self.git(&["init"], &[])?;
                if !out.ok() {
                    return Err(format!("git init 失败: {}", out.stderr));
                }
                let _ = self.git(&["checkout", "-B", &self.branch], &[]);
            }
            for (k, v) in GIT_IDENTITY {
                let out = self.git(&["config", k, v], &[])?;
                if !out.ok() {
                    return Err(format!("git config {k} 失败: {}", out.stderr));
                }
            }
        }
        Ok(())
    }

    /// 把数据目录中的同步文件（净化后）原子写入 sync/ 并提交（有变更才提交）。
    /// 返回（是否发生了 commit，commit message 或空）。
    pub fn write_and_commit(&self, epoch_secs: i64) -> Result<(bool, String), String> {
        let mut wrote_any = false;
        for name in SYNC_FILES {
            let src = self.data_dir.join(name);
            if !src.exists() {
                continue; // templates.json 等扩展点：存在才同步
            }
            let raw = std::fs::read_to_string(&src)
                .map_err(|e| format!("读取 {name} 失败: {e}"))?;
            let sanitized = sanitize_for_sync(&raw);
            let dst = self.sync_dir.join(name);
            atomic_write(&dst, sanitized.as_bytes())?;
            wrote_any = true;
        }
        if !wrote_any {
            return Ok((false, String::new()));
        }
        let st = self.git(&["status", "--porcelain"], &[])?;
        if !st.ok() {
            return Err(format!("git status 失败: {}", st.stderr));
        }
        if st.stdout.trim().is_empty() {
            return Ok((false, String::new())); // 无变更
        }
        let summary = self.pending_summary();
        let msg = build_commit_message(&summary, epoch_secs);
        let out = self.git(&["add", "-A"], &[])?;
        if !out.ok() {
            return Err(format!("git add 失败: {}", out.stderr));
        }
        let out = self.git(&["commit", "-m", &msg], &[])?;
        if !out.ok() {
            return Err(format!("git commit 失败: {}", out.stderr));
        }
        Ok((true, msg))
    }

    /// 未提交变更的摘要：对 sync/ 中的净化副本与 HEAD 版本做结构化对比。
    fn pending_summary(&self) -> String {
        for (name, unit) in [("connections.json", "连接"), ("groups.json", "分组")] {
            let Ok(new_text) = std::fs::read_to_string(self.sync_dir.join(name)) else { continue };
            let old_text = self
                .git(&["show", &format!("HEAD:{name}")], &[])
                .ok()
                .filter(|o| o.ok())
                .map(|o| o.stdout)
                .unwrap_or_else(|| "[]".into());
            if let Some(s) = summarize_json_arrays(&old_text, &new_text, unit) {
                return s;
            }
        }
        "数据变更".into()
    }

    /// 本地最后提交时间（epoch 秒；无提交 = 0）。
    pub fn last_commit_epoch(&self) -> i64 {
        self.git(&["log", "-1", "--format=%ct", "HEAD"], &[])
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| o.stdout.trim().parse::<i64>().ok())
            .unwrap_or(0)
    }

    /// sync 仓库是否有未提交变更。
    pub fn is_dirty(&self) -> bool {
        self.git(&["status", "--porcelain"], &[])
            .map(|o| o.ok() && !o.stdout.trim().is_empty())
            .unwrap_or(false)
    }

    /// push（先确保远端 URL 与分支上游）。仓库无远端（空 remote）时静默跳过。
    pub fn push(&self, remote: &str, auth: &SyncAuth) -> Result<(), String> {
        if remote.trim().is_empty() {
            return Ok(());
        }
        let url = auth.remote_url(remote);
        // 幂等设置 origin（URL 每次刷新，token 轮换即时生效）
        let _ = self.git(&["remote", "remove", "origin"], &[]);
        let out = self.git(&["remote", "add", "origin", &url], &[])?;
        if !out.ok() {
            return Err(format!("设置远端失败: {}", out.stderr));
        }
        let out = self.git(&["push", "-u", "origin", &self.branch], &auth.envs())?;
        if !out.ok() {
            return Err(format!("git push 失败: {}", out.stderr.trim()));
        }
        Ok(())
    }

    /// pull。返回（决策，远端 connections.json 原文（占位符仍在），
    /// 冲突时写入的 .bak 文件路径列表）。
    /// 决策为 [`PullDecision::RemoteNewer`] 时已执行 `reset --hard origin/<branch>`，
    /// 调用方读取 sync/ 中的副本并做占位符还原后覆盖数据目录。
    pub fn pull(
        &self,
        remote: &str,
        auth: &SyncAuth,
    ) -> Result<(PullDecision, Option<String>, Vec<PathBuf>), String> {
        if remote.trim().is_empty() {
            return Ok((PullDecision::RemoteEmpty, None, Vec::new()));
        }
        let url = auth.remote_url(remote);
        let _ = self.git(&["remote", "remove", "origin"], &[]);
        let out = self.git(&["remote", "add", "origin", &url], &[])?;
        if !out.ok() {
            return Err(format!("设置远端失败: {}", out.stderr));
        }
        let out = self.git(&["fetch", "origin", &self.branch], &auth.envs())?;
        if !out.ok() {
            return Err(format!("git fetch 失败: {}", out.stderr.trim()));
        }
        let refname = format!("origin/{}", self.branch);
        let remote_ts = self
            .git(&["log", "-1", "--format=%ct", &refname], &[])
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| o.stdout.trim().parse::<i64>().ok());
        let Some(remote_ts) = remote_ts else {
            return Ok((PullDecision::RemoteEmpty, None, Vec::new()));
        };
        let remote_conn = self
            .git(&["show", &format!("{refname}:connections.json")], &[])
            .ok()
            .filter(|o| o.ok())
            .map(|o| o.stdout);
        let remote_conn_count = remote_conn
            .as_deref()
            .map(count_placeholders)
            .unwrap_or(0);
        let local_ts = self.last_commit_epoch();
        let remote_newer = remote_ts > local_ts;
        let decision = decide_pull(remote_newer, self.is_dirty(), remote_conn_count);
        match decision {
            PullDecision::RemoteNewer { .. } => {
                let out = self.git(&["reset", "--hard", &refname], &[])?;
                if !out.ok() {
                    return Err(format!("git reset 失败: {}", out.stderr));
                }
                Ok((decision, remote_conn, Vec::new()))
            }
            PullDecision::ConflictLocalWins => {
                // 本地优先；远端各文件（净化后）存 .remote.bak 到数据目录
                let mut baks = Vec::new();
                for name in SYNC_FILES {
                    let blob = self.git(&["show", &format!("{refname}:{name}")], &[]).ok();
                    if let Some(o) = blob.filter(|o| o.ok()).map(|o| o.stdout) {
                        let dst = self.data_dir.join(format!("{name}.remote.bak"));
                        atomic_write(&dst, sanitize_for_sync(&o).as_bytes())?;
                        baks.push(dst);
                    }
                }
                Ok((decision, remote_conn, baks))
            }
            other => Ok((other, None, Vec::new())),
        }
    }

    /// 从 sync/ 读取某文件副本（reset --hard 之后调用）。
    pub fn synced_file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.sync_dir.join(name)).ok()
    }
}

/// 原子写：临时文件 + rename。
fn atomic_write(dst: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let tmp = dst.with_extension("tmp-sync");
    std::fs::write(&tmp, data).map_err(|e| format!("写临时文件失败: {e}"))?;
    // Windows 下 rename 目标已存在会失败，先删再 rename
    let _ = std::fs::remove_file(dst);
    std::fs::rename(&tmp, dst).map_err(|e| format!("原子替换失败: {e}"))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 脚本化 mock：按调用序回放输出列表；记录每次调用的 args。
    struct MockGit {
        script: Mutex<Vec<GitOut>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockGit {
        fn new(outs: Vec<GitOut>) -> Self {
            Self { script: Mutex::new(outs), calls: Mutex::new(Vec::new()) }
        }
        fn out(code: i32, stdout: &str) -> GitOut {
            GitOut { code, stdout: stdout.into(), stderr: String::new() }
        }
    }

    impl GitRunner for MockGit {
        fn run(&self, _cwd: &Path, args: &[&str], _envs: &[(String, String)]) -> Result<GitOut, String> {
            self.calls.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
            let mut q = self.script.lock().unwrap();
            if q.is_empty() {
                return Ok(GitOut { code: 0, stdout: String::new(), stderr: String::new() });
            }
            Ok(q.remove(0))
        }
    }

    // ---- 密文占位符 ----

    #[test]
    fn sanitize_replaces_dpapi_ciphertext_only() {
        let raw = r#"[
 {"id":"c1","profile":{"username":"dpapi:AAA=","password":"dpapi:BBB+/=="},"password":"dpapi:CCC="},
 {"id":"c2","profile":{"username":"dpapi 明文样例文本","password":""}}
]"#;
        let out = sanitize_for_sync(raw);
        assert!(out.contains("\"dpapi:<不可迁移>\""), "占位符应带引号保留 JSON 结构: {out}");
        assert!(!out.contains("AAA="));
        assert!(!out.contains("BBB+/=="));
        assert!(!out.contains("CCC="));
        // 普通字符串中的 "dpapi " 字样不受影响
        assert!(out.contains("dpapi 明文样例文本"));
        assert_eq!(count_placeholders(&out), 3);
    }

    #[test]
    fn sanitize_preserves_non_string_dpapi_occurrence() {
        // "userdpapi:xxx" 前一个字节不是引号开头（在字符串中间），不应被替换
        let raw = r#"{"name":"mydpapi:abc"}"#;
        let out = sanitize_for_sync(raw);
        assert!(out.contains("mydpapi:abc"));
        assert_eq!(count_placeholders(&out), 0);
    }

    #[test]
    fn restore_clears_placeholders_and_counts() {
        let synced = r#"[{"id":"c1","password":"dpapi:<不可迁移>","gateway":{"token":"dpapi:<不可迁移>"}}]"#;
        let (restored, n) = restore_from_sync(synced);
        assert_eq!(n, 2);
        assert!(restored.contains("\"password\":\"\""));
        assert!(restored.contains("\"token\":\"\""));
        assert!(!restored.contains("<不可迁移>"));
    }

    // ---- 变更摘要 ----

    #[test]
    fn summarize_added_changed_removed() {
        let old = r#"[{"id":"a","name":"A"},{"id":"b","name":"B"},{"id":"c","name":"C"}]"#;
        let new = r#"[{"id":"a","name":"A"},{"id":"b","name":"B2"},{"id":"d","name":"D"}]"#;
        let s = summarize_json_arrays(old, new, "连接").unwrap();
        assert!(s.contains("新增 1"), "{s}");
        assert!(s.contains("修改 1"), "{s}");
        assert!(s.contains("删除 1"), "{s}");
        assert!(s.contains("连接"), "{s}");
    }

    #[test]
    fn summarize_none_when_identical() {
        let old = r#"[{"id":"a","name":"A"}]"#;
        assert!(summarize_json_arrays(old, old, "连接").is_none());
    }

    #[test]
    fn summarize_tolerates_broken_old() {
        let s = summarize_json_arrays("not json", r#"[{"id":"a"}]"#, "连接").unwrap();
        assert!(s.contains("新增 1"));
    }

    // ---- commit message ----

    #[test]
    fn commit_message_format() {
        // 2026-01-01 00:00:00 UTC = epoch 1767225600
        let msg = build_commit_message("新增 2，删除 1（连接）", 1_767_225_600);
        assert_eq!(msg, "sync: 新增 2，删除 1（连接） @ 2026-01-01 00:00:00 UTC");
    }

    #[test]
    fn timestamp_known_anchor() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc_timestamp(951_782_400), "2000-02-29 00:00:00"); // 闰年
    }

    // ---- pull 冲突策略 ----

    #[test]
    fn pull_decision_matrix() {
        assert_eq!(decide_pull(false, false, 0), PullDecision::UpToDate);
        assert_eq!(decide_pull(false, true, 0), PullDecision::LocalAhead);
        assert_eq!(
            decide_pull(true, false, 5),
            PullDecision::RemoteNewer { restored: 5 }
        );
        assert_eq!(decide_pull(true, true, 5), PullDecision::ConflictLocalWins);
    }

    // ---- 认证 URL ----

    #[test]
    fn token_url_embedding() {
        let auth = SyncAuth::HttpsToken("tok123".into());
        assert_eq!(
            auth.remote_url("https://git.example.com/u/r.git"),
            "https://x-access-token:tok123@git.example.com/u/r.git"
        );
        assert_eq!(
            auth.remote_url("git@github.com:u/r.git"),
            "git@github.com:u/r.git"
        );
        assert_eq!(SyncAuth::None.remote_url("https://h/r.git"), "https://h/r.git");
    }

    #[test]
    fn ssh_env_command() {
        let auth = SyncAuth::SshKey(PathBuf::from("C:/keys/id_ed25519"));
        let envs = auth.envs();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].0, "GIT_SSH_COMMAND");
        assert!(envs[0].1.contains("-i \"C:/keys/id_ed25519\""));
        assert!(envs[0].1.contains("StrictHostKeyChecking=accept-new"));
    }

    // ---- SyncManager 交互（mock git） ----

    fn tmp_dir(tag: &str) -> PathBuf {
        // 用 workspace 本地目录（%TEMP% 在受限环境下可能拒绝创建）
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-tmp");
        let d = base.join(format!(
            "gitsync-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_and_commit_skips_when_no_files() {
        let dir = tmp_dir("clean");
        let mock = MockGit::new(vec![]);
        // 数据目录无任何同步文件 → 不写不提交
        let m = SyncManager::new(&mock, dir.clone(), "main");
        let (committed, _msg) = m.write_and_commit(0).unwrap();
        assert!(!committed);
        assert!(mock.calls.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_and_commit_commits_with_message() {
        let dir = tmp_dir("commit");
        std::fs::write(
            dir.join("connections.json"),
            r#"[{"id":"c1","password":"dpapi:SECT"}]"#,
        )
        .unwrap();
        let mock = MockGit::new(vec![
            MockGit::out(0, " M connections.json\n"), // status：有变更
            MockGit::out(0, ""),                      // show HEAD:connections.json（旧版为空）
            MockGit::out(0, ""),                      // add
            MockGit::out(0, "[main abc] sync: ..."),  // commit
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        let (committed, msg) = m.write_and_commit(0).unwrap();
        assert!(committed);
        assert!(msg.starts_with("sync: "), "{msg}");
        assert!(msg.contains("@ ") && msg.ends_with(" UTC"));
        // 副本内容已净化
        let copy = std::fs::read_to_string(dir.join("sync").join("connections.json")).unwrap();
        assert!(copy.contains(CRED_PLACEHOLDER));
        assert!(!copy.contains("SECT"));
        // 调用序列含 add/commit
        let calls = mock.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c.join(" ") == "add -A"));
        assert!(calls.iter().any(|c| c.first().map(String::as_str) == Some("commit")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pull_remote_newer_resets_hard() {
        let dir = tmp_dir("pullnew");
        std::fs::create_dir_all(dir.join("sync")).unwrap();
        let mock = MockGit::new(vec![
            MockGit::out(0, ""),             // remote remove
            MockGit::out(0, ""),             // remote add
            MockGit::out(0, ""),             // fetch
            MockGit::out(0, "2000000000\n"), // log %ct origin/main（远端较新）
            MockGit::out(0, "1990000000\n"), // log %ct HEAD
            MockGit::out(0, "?? x\n"),       // status（dirty）
            MockGit::out(0, ""),             // reset --hard
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        let (decision, conn, baks) = m.pull("https://git.example.com/r.git", &SyncAuth::None).unwrap();
        assert_eq!(decision, PullDecision::RemoteNewer { restored: 0 });
        // 远端 connections.json 原文（mock 回放默认空串，占位符仍在其中）
        assert!(conn.is_some());
        assert!(baks.is_empty());
        let calls = mock.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c.first().map(String::as_str) == Some("reset")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pull_conflict_writes_remote_bak() {
        let dir = tmp_dir("pullconf");
        let remote_conn = r#"[{"id":"r1","password":"dpapi:REMOTE"}]"#;
        let mock = MockGit::new(vec![
            MockGit::out(0, ""),             // remote remove
            MockGit::out(0, ""),             // remote add
            MockGit::out(0, ""),             // fetch
            MockGit::out(0, "2000000000\n"), // origin/main %ct（远端较新）
            MockGit::out(0, remote_conn),    // show origin/main:connections.json
            MockGit::out(0, "1900000000\n"), // HEAD %ct（远端较新成立）
            MockGit::out(0, " M connections.json\n"), // status dirty → 冲突
            MockGit::out(0, remote_conn),    // show（bak：connections.json）
            MockGit::out(128, ""),           // show groups.json（远端不存在）
            MockGit::out(128, ""),           // show templates.json（远端不存在）
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        let (decision, _conn, baks) = m.pull("https://h/r.git", &SyncAuth::None).unwrap();
        assert_eq!(decision, PullDecision::ConflictLocalWins);
        assert_eq!(baks.len(), 1);
        let bak = std::fs::read_to_string(&baks[0]).unwrap();
        assert!(bak.contains(CRED_PLACEHOLDER), "bak 也要净化: {bak}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pull_remote_empty_when_branch_missing() {
        let dir = tmp_dir("pullmt");
        std::fs::create_dir_all(dir.join("sync")).unwrap();
        let mock = MockGit::new(vec![
            MockGit::out(0, ""),   // remote remove
            MockGit::out(0, ""),   // remote add
            MockGit::out(0, ""),   // fetch（远端空仓库也返回 0）
            MockGit::out(128, ""), // log origin/main 不存在
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        let (decision, _, _) = m.pull("https://h/r.git", &SyncAuth::None).unwrap();
        assert_eq!(decision, PullDecision::RemoteEmpty);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_repo_runs_init_and_config_when_missing() {
        let dir = tmp_dir("init");
        let mock = MockGit::new(vec![
            MockGit::out(0, ""), // init -b main
            MockGit::out(0, ""), // config user.name
            MockGit::out(0, ""), // config user.email
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        m.ensure_repo().unwrap();
        let calls = mock.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c.join(" ") == "init -b main"));
        assert!(calls.iter().any(|c| c.first().map(String::as_str) == Some("config")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn push_sets_remote_and_pushes() {
        let dir = tmp_dir("push");
        std::fs::create_dir_all(dir.join("sync")).unwrap();
        let mock = MockGit::new(vec![
            MockGit::out(0, ""), // remote remove
            MockGit::out(0, ""), // remote add
            MockGit::out(0, ""), // push
        ]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        m.push("https://h/r.git", &SyncAuth::HttpsToken("t".into())).unwrap();
        let calls = mock.calls.lock().unwrap();
        let add = calls
            .iter()
            .find(|c| c.first().map(String::as_str) == Some("remote") && c.get(1).map(String::as_str) == Some("add"))
            .unwrap();
        assert!(add[3].contains("x-access-token:t@"));
        assert!(calls.iter().any(|c| c.first().map(String::as_str) == Some("push")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn push_skips_empty_remote() {
        let dir = tmp_dir("pushskip");
        std::fs::create_dir_all(dir.join("sync")).unwrap();
        let mock = MockGit::new(vec![]);
        let m = SyncManager::new(&mock, dir.clone(), "main");
        m.push("  ", &SyncAuth::None).unwrap();
        assert!(mock.calls.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
