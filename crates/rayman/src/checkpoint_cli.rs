use anyhow::{Result, bail};
use serde_json::json;

use crate::cli::{CheckpointAction, CheckpointCmd};
use crate::i18n;
use rayman::checkpoint;

pub fn run_checkpoint(root: &std::path::Path, json: bool, cmd: CheckpointCmd) -> Result<()> {
    let dir = cmd.dir.as_deref();
    match cmd.action {
        CheckpointAction::Save { keep } => {
            let outcome = match keep {
                Some(keep) => checkpoint::save(root, dir, keep)?,
                None => checkpoint::save_without_prune(root, dir)?,
            };
            let mb = outcome.total_bytes as f64 / 1_048_576.0;
            if json {
                crate::print(&json!({
                    "id": outcome.id,
                    "path": rayman::pathfmt::display_path(&outcome.path),
                    "file_count": outcome.file_count,
                    "skipped_count": outcome.skipped_count,
                    "total_bytes": outcome.total_bytes,
                    "pruned": outcome.pruned,
                    "purpose": outcome.purpose,
                    "retention_applied": keep.is_some(),
                }));
            } else {
                println!(
                    "已保存快照 {} — {} 个文件 ({:.1} MB){}{}",
                    outcome.id,
                    outcome.file_count,
                    mb,
                    // `save` only ever returns a complete snapshot; anything
                    // skipped makes it a partial one, which is reported as an
                    // error instead of an outcome. Printing a skip count here
                    // described a state this branch can never observe.
                    String::new(),
                    match keep {
                        Some(_) =>
                            format!("，按显式 retention policy 清理旧快照 {} 个", outcome.pruned),
                        None => "；未裁剪任何旧恢复点".to_string(),
                    }
                );
                println!("  位置: {}", rayman::pathfmt::display_path(&outcome.path));
            }
        }
        CheckpointAction::Prune { keep, yes } => {
            if !yes {
                bail!("checkpoint prune 会删除旧恢复点；传 --yes 显式确认");
            }
            let keep = keep.max(1);
            let pruned = checkpoint::prune_standard_snapshots(root, dir, keep)?;
            if json {
                crate::print(&json!({ "keep": keep, "pruned": pruned }));
            } else {
                println!(
                    "已按显式 retention policy 保留最近 {} 个完整快照，删除 {} 个",
                    keep, pruned
                );
            }
        }
        CheckpointAction::SalvageSave => {
            let outcome = checkpoint::salvage_save(root, dir)?;
            if json {
                crate::print(&json!({
                    "id": outcome.id,
                    "path": rayman::pathfmt::display_path(&outcome.path),
                    "file_count": outcome.file_count,
                    "total_bytes": outcome.total_bytes,
                    "pruned": outcome.pruned,
                    "purpose": outcome.purpose,
                    "authoritative": false,
                }));
            } else {
                println!(
                    "已保存 recovery-only 快照 {} — {} 个文件；它不会成为默认 latest 或完成证据",
                    outcome.id, outcome.file_count
                );
                println!("  位置: {}", rayman::pathfmt::display_path(&outcome.path));
            }
        }
        CheckpointAction::List => {
            let checkpoints = checkpoint::list(root, dir)?;
            if json {
                let items: Vec<_> = checkpoints
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "status": c.status,
                            "purpose": c.manifest.as_ref().map(|m| m.purpose),
                            "activation_status": c.manifest.as_ref().map(|m| m.activation.status.clone()),
                            "created_at": c.manifest.as_ref().map(|m| m.created_at.clone()),
                            "file_count": c.manifest.as_ref().map(|m| m.file_count),
                            "total_bytes": c.manifest.as_ref().map(|m| m.total_bytes),
                        })
                    })
                    .collect();
                crate::print(&serde_json::to_value(&items)?);
            } else if checkpoints.is_empty() {
                println!("当前工作区暂无快照。运行 `rayman checkpoint save` 创建一个。");
            } else {
                println!("快照（旧→新）:");
                for c in &checkpoints {
                    match &c.manifest {
                        Some(m) => println!(
                            "  {}  {:?}/{:?}  {} 个文件  {:.1} MB",
                            c.id,
                            c.status,
                            m.purpose,
                            m.file_count,
                            m.total_bytes as f64 / 1_048_576.0
                        ),
                        None => println!("  {}  {:?}  (缺或损坏 manifest)", c.id, c.status),
                    }
                }
            }
        }
        CheckpointAction::Status => {
            let latest = checkpoint::latest(root, dir)?;
            // `latest` deliberately means "restorable standard snapshot", but
            // reporting "no snapshots" while `list` shows recovery-only or
            // partial ones told the user their captures did not exist. Count
            // what is actually on disk so the two commands cannot contradict.
            let other = if latest.is_none() {
                checkpoint::list(root, dir).map(|all| all.len()).unwrap_or(0)
            } else {
                0
            };
            if json {
                crate::print(&json!({
                    "has_checkpoint": latest.is_some(),
                    "non_standard_snapshots": other,
                    "latest": latest.as_ref().map(|c| json!({
                        "id": c.id,
                        "status": c.status,
                        "purpose": c.manifest.as_ref().map(|m| m.purpose),
                        "created_at": c.manifest.as_ref().map(|m| m.created_at.clone()),
                        "file_count": c.manifest.as_ref().map(|m| m.file_count),
                    })),
                }));
            } else {
                match latest {
                    Some(c) => {
                        let created = c
                            .manifest
                            .as_ref()
                            .map(|m| m.created_at.clone())
                            .unwrap_or_else(|| "?".to_string());
                        println!(
                            "{}",
                            i18n::message(
                                i18n::MessageId::CheckpointStatus,
                                &[c.id, format!("{:?}", c.status), created],
                            )
                        );
                    }
                    None if other > 0 => println!(
                        "当前工作区没有可恢复的 standard 快照；另有 {other} 份 recovery-only/partial 快照，用 `rayman checkpoint list` 查看。"
                    ),
                    None => println!("当前工作区暂无快照。"),
                }
            }
        }
        CheckpointAction::Restore {
            id,
            yes,
            allow_recovery_only,
        } => {
            if !yes {
                bail!(
                    "恢复会用快照覆盖工作区里的同名文件。确认请加 --yes：rayman checkpoint restore --yes"
                );
            }
            let outcome =
                checkpoint::restore_with_options(root, dir, id.as_deref(), allow_recovery_only)?;
            if json {
                crate::print(&json!({
                    "id": outcome.id,
                    "restored": outcome.restored,
                    "failed": outcome.failed,
                }));
            } else {
                println!(
                    "已从快照 {} 恢复 {} 个文件{}。",
                    outcome.id,
                    outcome.restored,
                    if outcome.failed > 0 {
                        format!("，{} 个失败", outcome.failed)
                    } else {
                        String::new()
                    }
                );
            }
        }
        CheckpointAction::Verify { id } => {
            let checkpoints = checkpoint::list(root, dir)?;
            let target = match id.as_deref() {
                None | Some("latest") => checkpoint::latest(root, dir)?,
                Some(id) => checkpoints
                    .into_iter()
                    .find(|checkpoint| checkpoint.id == id),
            }
            .ok_or_else(|| anyhow::anyhow!("找不到可验证的 checkpoint"))?;
            let manifest = checkpoint::verify_snapshot(&target.path)?;
            if json {
                crate::print(&json!({
                    "id": target.id,
                    "status": "complete",
                    "file_count": manifest.file_count,
                    "total_bytes": manifest.total_bytes,
                    "schema": manifest.schema,
                    "version": manifest.version,
                    "purpose": manifest.purpose,
                    "activation": manifest.activation,
                }));
            } else {
                println!(
                    "checkpoint {} 已验证：{} 个文件，{:.1} MB",
                    target.id,
                    manifest.file_count,
                    manifest.total_bytes as f64 / 1_048_576.0
                );
            }
        }
    }
    Ok(())
}
