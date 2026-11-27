//! `linkdesk schema [OUT_FILE]` — 输出 EasyRDP JSON Profile 的 JSON Schema
//! （draft-07）。仓库无独立 CLI 二进制，按 R-20261118-009 约定落在
//! easyrdp-core example；后续若补全 linkdesk CLI，可直接把本文件的
//! schema 子命令挂入现有入口。
//!
//! 用法：
//!   cargo run -p easyrdp-core --example linkdesk -- schema                # 打印到 stdout
//!   cargo run -p easyrdp-core --example linkdesk -- schema profile.schema.json
//!
//! 可选：`validate <profile.json> [--strict]` 对单个 JSON Profile 执行导入前校验。

use easyrdp_core::schema::{profile_json_schema, validate_profile_json, ValidationMode};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("schema") => {
            let out = args.get(1);
            let text = profile_json_schema();
            match out {
                Some(path) => match std::fs::write(path, text) {
                    Ok(()) => {
                        println!("已写入 JSON Profile Schema -> {path}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("写入 {path} 失败: {e}");
                        ExitCode::FAILURE
                    }
                },
                None => {
                    println!("{text}");
                    ExitCode::SUCCESS
                }
            }
        }
        Some("validate") => {
            let Some(path) = args.get(1) else {
                eprintln!("用法: linkdesk validate <profile.json> [--strict]");
                return ExitCode::FAILURE;
            };
            let strict = args.iter().any(|a| a == "--strict");
            let mode = if strict { ValidationMode::Strict } else { ValidationMode::Lenient };
            let raw = match std::fs::read_to_string(path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("读取 {path} 失败: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let v: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("JSON 解析失败: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match validate_profile_json(&v, mode) {
                Ok(warnings) => {
                    for w in &warnings {
                        println!("警告: {w}");
                    }
                    println!("校验通过（{} 条警告）", warnings.len());
                    ExitCode::SUCCESS
                }
                Err(issues) => {
                    for i in &issues {
                        eprintln!("{i}");
                    }
                    eprintln!("校验失败（{} 条问题）", issues.len());
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("用法: linkdesk schema [OUT_FILE] | linkdesk validate <profile.json> [--strict]");
            ExitCode::FAILURE
        }
    }
}
