// F6-V2（R-20261118-015）Git 同步：Tauri 命令与自动同步挂钩。
//
// 职责：
//   1. 读取 settings.json 中的 gitSync* 配置（token 为 DPAPI 密文，
//      使用前 unprotect；密文本机可解、跨机器不可解）；
//   2. git_sync_status / git_sync_push / git_sync_pull 三个命令；
//   3. auto_sync_after_change：ConnectionStore 每次落盘后的自动 commit+push
//      （auto_commit = "each_change" 且开关打开时；后台线程静默执行）。
//
// 安全边界（与 crates/easyrdp-core/src/git_sync.rs 的语义一致）：
//   - 进入 sync/ 仓库的副本已把 dpapi: 密文替换为 `dpapi:<不可迁移>`；
//   - pull 恢复时占位符字段清空（""），由用户重填凭据；
//   - git 远端 token 仅存在于 settings.json（DPAPI 密文），commit/push 命令
//     行参数不携带（HTTPS token 走远端 URL 时由 git 进程内存中拼接，
//     不落盘到仓库配置之外的文件……注：remote origin URL 含 token，
//     位于 sync/.git/config（本机数据目录内），属可接受 MVP 权衡，见报告）。

use easyrdp_core::git_sync::{
    git_available, restore_from_sync, SyncAuth, SyncManager, RealGitRunner, PullDecision,
};
use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ConnectionStore;

const K_ENABLED: &str = "gitSyncEnabled";
const K_REMOTE: &str = "gitSyncRemote";
const K_BRANCH: &str = "gitSyncBranch";
const K_AUTH: &str = "gitSyncAuthMode"; // "https_token" | "ssh_key"
const K_TOKEN: &str = "gitSyncToken"; // settings.json 中为 dpapi: 密文
const K_SSH: &str = "gitSyncSshKey";
const K_AUTO: &str = "gitSyncAutoCommit"; // "each_change" | "manual"

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// 读取并解析同步配置；enabled=false 时返回 None。
fn read_config(app: &AppHandle) -> Result<Option<(SyncSettings, PathBuf)>, String> {
    let map = crate::settings::load_settings_map_public(app);
    let dir = data_dir(app)?;
    let s = SyncSettings {
        enabled: map.get(K_ENABLED).and_then(|v| v.as_bool()).unwrap_or(false),
        remote: map.get(K_REMOTE).and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
        branch: {
            let b = map.get(K_BRANCH).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if b.is_empty() { "main".to_string() } else { b }
        },
        auth_mode: map.get(K_AUTH).and_then(|v| v.as_str()).unwrap_or("https_token").to_string(),
        ssh_key: map.get(K_SSH).and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
        token_plain: {
            let stored = map.get(K_TOKEN).and_then(|v| v.as_str()).unwrap_or("");
            if stored.is_empty() {
                None
            } else {
                // settings.json 中是 dpapi: 密文；解密失败（跨用户等）按无 token 处理
                easyrdp_core::secret::unprotect(stored).ok()
            }
        },
        auto: map.get(K_AUTO).and_then(|v| v.as_str()).unwrap_or("each_change") == "each_change",
    };
    if !s.enabled {
        return Ok(None);
    }
    Ok(Some((s, dir)))
}

struct SyncSettings {
    enabled: bool,
    remote: String,
    branch: String,
    auth_mode: String,
    ssh_key: String,
    token_plain: Option<String>,
    auto: bool,
}

impl SyncSettings {
    fn auth(&self) -> SyncAuth {
        match self.auth_mode.as_str() {
            "ssh_key" => SyncAuth::SshKey(PathBuf::from(&self.ssh_key)),
            _ => SyncAuth::HttpsToken(self.token_plain.clone().unwrap_or_default()),
        }
    }
}

fn manager<'a>(runner: &'a RealGitRunner, dir: PathBuf, branch: &str) -> SyncManager<'a> {
    SyncManager::new(runner, dir, branch)
}

/// 同步状态（设置页回显）。
#[derive(Serialize)]
pub struct SyncStatus {
    git_available: bool,
    enabled: bool,
    remote: String,
    branch: String,
    auth_mode: String,
    has_token: bool,
    ssh_key: String,
    auto_commit: bool,
    repo_ready: bool,
    last_commit: String,
    dirty: bool,
}

#[tauri::command]
pub fn git_sync_status(app: AppHandle) -> SyncStatus {
    let avail = git_available();
    let (cfg, dir) = match read_config(&app) {
        Ok(Some(v)) => v,
        _ => {
            return SyncStatus {
                git_available: avail,
                enabled: false,
                remote: String::new(),
                branch: "main".into(),
                auth_mode: "https_token".into(),
                has_token: false,
                ssh_key: String::new(),
                auto_commit: true,
                repo_ready: false,
                last_commit: String::new(),
                dirty: false,
            }
        }
    };
    let runner = RealGitRunner;
    let m = manager(&runner, dir.clone(), &cfg.branch);
    let repo_ready = dir.join("sync").join(".git").exists();
    let last_commit = if repo_ready {
        let epoch = m.last_commit_epoch();
        if epoch > 0 {
            easyrdp_core::git_sync::format_utc_timestamp(epoch)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    SyncStatus {
        git_available: avail,
        enabled: true,
        remote: cfg.remote,
        branch: cfg.branch,
        auth_mode: cfg.auth_mode,
        has_token: cfg.token_plain.is_some(),
        ssh_key: cfg.ssh_key,
        auto_commit: cfg.auto,
        repo_ready,
        last_commit,
        dirty: if repo_ready { m.is_dirty() } else { false },
    }
}

/// 立即同步：把当前数据净化写入 sync/ 仓库，有变更则 commit 并 push。
/// 返回结果消息（前端 status 区展示）。
#[tauri::command]
pub fn git_sync_push(app: AppHandle, summary: Option<String>) -> Result<String, String> {
    if !git_available() {
        return Err("未检测到系统 git（请安装 Git 并加入 PATH）".into());
    }
    let Some((cfg, dir)) = read_config(&app)? else {
        return Err("Git 同步未启用".into());
    };
    let runner = RealGitRunner;
    let m = manager(&runner, dir.clone(), &cfg.branch);
    m.ensure_repo()?;
    let (committed, msg) = m.write_and_commit(now_epoch())?;
    if committed {
        m.push(&cfg.remote, &cfg.auth())?;
        let _ = summary;
        Ok(format!("已提交并推送：{msg}"))
    } else {
        m.push(&cfg.remote, &cfg.auth())?;
        Ok("本地无变更（已确保远端同步）".into())
    }
}

/// pull 结果。
#[derive(Serialize)]
pub struct PullReport {
    /// empty | up_to_date | local_ahead | remote_newer | conflict_local_wins
    status: String,
    restored: usize,
    message: String,
    /// 冲突时远端版本备份文件路径。
    backups: Vec<String>,
}

#[tauri::command]
pub fn git_sync_pull(app: AppHandle, store: State<'_, ConnectionStore>) -> Result<PullReport, String> {
    if !git_available() {
        return Err("未检测到系统 git（请安装 Git 并加入 PATH）".into());
    }
    let Some((cfg, dir)) = read_config(&app)? else {
        return Err("Git 同步未启用".into());
    };
    let runner = RealGitRunner;
    let m = manager(&runner, dir.clone(), &cfg.branch);
    m.ensure_repo()?;
    let (decision, _remote_conn, baks) = m.pull(&cfg.remote, &cfg.auth())?;
    let report = match decision {
        PullDecision::RemoteEmpty => PullReport {
            status: "empty".into(),
            restored: 0,
            message: "远端仓库为空（尚无远端提交）".into(),
            backups: vec![],
        },
        PullDecision::UpToDate => PullReport {
            status: "up_to_date".into(),
            restored: 0,
            message: "本地与远端一致".into(),
            backups: vec![],
        },
        PullDecision::LocalAhead => {
            // 本地领先：写入变更并推送
            let (committed, msg) = m.write_and_commit(now_epoch())?;
            if committed {
                m.push(&cfg.remote, &cfg.auth())?;
            }
            PullReport {
                status: "local_ahead".into(),
                restored: 0,
                message: if committed { format!("本地领先，已推送：{msg}") } else { "本地领先且已推送".into() },
                backups: vec![],
            }
        }
        PullDecision::RemoteNewer { .. } => {
            // 远端较新且本地干净：reset --hard 已执行，逐文件还原并覆盖数据目录
            let mut restored_conns = 0usize;
            for name in easyrdp_core::git_sync::SYNC_FILES {
                let Some(raw) = m.synced_file(name) else { continue };
                let (clean, _) = restore_from_sync(&raw);
                let dst = dir.join(name);
                std::fs::write(&dst, &clean).map_err(|e| format!("覆盖 {name} 失败: {e}"))?;
                if name == "connections.json" {
                    // 恢复条数：解析数组长度，失败回退占位符计数
                    restored_conns = serde_json::from_str::<serde_json::Value>(&clean)
                        .ok()
                        .and_then(|v| v.as_array().map(|a| a.len()))
                        .unwrap_or_else(|| easyrdp_core::git_sync::count_placeholders(&raw));
                }
            }
            // 内存态重新从磁盘加载
            store.reload_from_disk();
            let _ = app.emit("easyrdp-sync-pulled", restored_conns);
            PullReport {
                status: "remote_newer".into(),
                restored: restored_conns,
                message: format!("已从远端恢复 {restored_conns} 条连接（凭据占位符已清空，请重新填写）"),
                backups: vec![],
            }
        }
        PullDecision::ConflictLocalWins => PullReport {
            status: "conflict_local_wins".into(),
            restored: 0,
            message: format!(
                "检测到冲突：已保留本地版本；远端版本已另存 {} 个 .remote.bak 文件（凭据同样已净化）",
                baks.len()
            ),
            backups: baks.iter().map(|p| p.display().to_string()).collect(),
        },
    };
    Ok(report)
}

/// ConnectionStore 每次落盘后的自动同步（后台线程，静默容错）。
/// enabled 且 auto_commit = each_change 时执行：净化写副本 → commit → push。
pub fn auto_sync_after_change(app: &AppHandle, summary: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            if !git_available() {
                return Ok(()); // git 缺失：静默跳过
            }
            let Some((cfg, dir)) = read_config(&app)? else { return Ok(()) };
            if !cfg.auto {
                return Ok(()); // 手动粒度：等用户点「立即同步」
            }
            let runner = RealGitRunner;
            let m = manager(&runner, dir.clone(), &cfg.branch);
            m.ensure_repo()?;
            let (committed, _msg) = m.write_and_commit(now_epoch())?;
            if committed {
                m.push(&cfg.remote, &cfg.auth())?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[git-sync] 自动同步失败: {e}");
        }
        let _ = summary;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pull 报告结构序列化（前端契约）字段齐全。
    #[test]
    fn pull_report_fields() {
        let r = PullReport {
            status: "remote_newer".into(),
            restored: 3,
            message: "已从远端恢复 3 条连接".into(),
            backups: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["status"], "remote_newer");
        assert_eq!(v["restored"], 3);
        assert!(v["message"].is_string());
        assert!(v["backups"].is_array());
    }
}
