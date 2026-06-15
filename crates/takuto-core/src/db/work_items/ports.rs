// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::*;

// ── work_item_port_mappings ──────────────────────────────────────────────

/// Insert or update a port mapping. Composite uniqueness on
/// `(work_item_id, container_port, kind)` is enforced in app code via
/// "delete then insert" (the schema has a surrogate `id` PK; uniqueness
/// would require a partial index with `COALESCE(run_command_index, -1)`
/// which is awkward cross-backend).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_port_mapping(
    adapter: &DbAdapter,
    work_item_id: &str,
    container_port: i32,
    host_port: i32,
    proxy_url: &str,
    path_token: &str,
    kind: PortMappingKind,
    run_command_index: Option<i32>,
    created_at: i64,
) -> Result<()> {
    // Wipe any existing mapping for this (work_item, port, kind).
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings \
             WHERE work_item_id = ? AND container_port = ? AND kind = ?",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    adapter
        .execute(
            "INSERT INTO work_item_port_mappings \
                (work_item_id, container_port, host_port, proxy_url, path_token, kind, \
                 run_command_index, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::I32(host_port),
                DbValue::Text(proxy_url.to_string()),
                DbValue::Text(path_token.to_string()),
                DbValue::Text(kind.as_str().to_string()),
                DbValue::I32Opt(run_command_index),
                DbValue::I64(created_at),
            ],
        )
        .await?;
    Ok(())
}

/// Delete a specific port mapping.
pub async fn delete_port_mapping(
    adapter: &DbAdapter,
    work_item_id: &str,
    container_port: i32,
    kind: PortMappingKind,
) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings \
             WHERE work_item_id = ? AND container_port = ? AND kind = ?",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Delete every port mapping for a work item, regardless of kind.
/// Used at editor close to wipe the editor + static + dynamic
/// rows in one shot; cheaper than per-(port, kind) deletes and
/// avoids leaking rows when a route handler forgot one.
pub async fn delete_port_mappings_for_work_item(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings WHERE work_item_id = ?",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    Ok(())
}

/// Shadow-write a port-mapping registration. Wraps
/// [`upsert_port_mapping`] with the standard shadow contract: `None`
/// `db` short-circuits, errors WARN and never propagate.
#[allow(clippy::too_many_arguments)]
pub async fn shadow_upsert_port_mapping(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
    container_port: i32,
    host_port: i32,
    proxy_url: &str,
    path_token: &str,
    kind: PortMappingKind,
    run_command_index: Option<i32>,
    created_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) = upsert_port_mapping(
        db.adapter(),
        work_item_id,
        container_port,
        host_port,
        proxy_url,
        path_token,
        kind,
        run_command_index,
        created_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            container_port,
            host_port,
            kind = %kind.as_str(),
            error = %e,
            "Ushadow-write of port mapping upsert failed (route handler progress unaffected)"
        );
    }
}

/// Shadow-clean every port mapping for a work item. Used at editor
/// close so the DB row mirrors the in-memory `path_token_registry`
/// cleanup.
pub async fn shadow_delete_port_mappings_for_work_item(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
) {
    let Some(db) = db else { return };
    if let Err(e) = delete_port_mappings_for_work_item(db.adapter(), work_item_id).await {
        tracing::warn!(
            work_item_id,
            error = %e,
            "Ushadow-clean of port mappings failed (route handler progress unaffected)"
        );
    }
}

/// List all port mappings for a work item.
pub async fn list_port_mappings(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<Vec<PortMappingRow>> {
    let rows = adapter
        .query_all(
            "SELECT id, work_item_id, container_port, host_port, proxy_url, path_token, \
                    kind, run_command_index, created_at \
             FROM work_item_port_mappings WHERE work_item_id = ? \
             ORDER BY kind ASC, container_port ASC",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let kind_s = r.get_text(6)?;
        let kind = PortMappingKind::from_str(&kind_s).map_err(|e| {
            crate::error::TakutoError::Db(crate::db::DbError::Adapter(
                crate::db::adapter::DbError::Sqlx {
                    source: sqlx::Error::Configuration(e.into()),
                },
            ))
        })?;
        out.push(PortMappingRow {
            id: r.get_i64(0)?,
            work_item_id: r.get_text(1)?,
            container_port: r.get_i64(2)? as i32,
            host_port: r.get_i64(3)? as i32,
            proxy_url: r.get_text(4)?,
            path_token: r.get_text(5)?,
            kind,
            run_command_index: r.get_i64_opt(7)?.map(|v| v as i32),
            created_at: r.get_i64(8)?,
        });
    }
    Ok(out)
}
