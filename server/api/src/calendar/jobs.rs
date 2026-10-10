//! Feed polling: bounded parallel network work without source locks. Each source
//! is rechecked under its lock before committing; failed fetches never delete items.
use super::*;
use crate::items::ics_import::apply_calendar;
use futures_util::{StreamExt, stream};
use porchlight_db::tx::{Actor, begin_as};
use sqlx::{Acquire, PgPool};

fn informational(w: &Warning) -> bool {
    matches!(
        w.code.as_str(),
        "ExpandedRule" | "DurationNormalized" | "WarningsTruncated" | "UnsupportedOverrideDetails"
    )
}
fn needs_unconditional_fetch(warnings: &[Warning]) -> bool {
    warnings
        .iter()
        .any(|w| matches!(w.code.as_str(), "ExpansionLimited" | "ExpandedRule"))
}
fn warning_status(warnings: &[Warning]) -> &'static str {
    if warnings.iter().any(|w| !informational(w)) {
        "partial"
    } else {
        "ok"
    }
}
fn fetch_warning_state(
    warnings: Vec<Warning>,
    etag: &mut Option<String>,
    modified: &mut Option<String>,
) -> (&'static str, Vec<Warning>) {
    // Neither an expansion budget nor a rolling projection horizon is a stable
    // file property. Expanded calendars must be reprojected even when unchanged.
    if needs_unconditional_fetch(&warnings) {
        *etag = None;
        *modified = None;
    }
    (warning_status(&warnings), bounded_warnings(warnings))
}
fn clean_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !matches!(*c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
        .take(max)
        .collect()
}
fn bounded_warnings(mut warnings: Vec<Warning>) -> Vec<Warning> {
    const LIMIT: usize = 50;
    if warnings.len() > LIMIT {
        let total = warnings.len();
        // Keep both expansion signals AND partial state when truncating. They
        // control future conditional fetching, including pre-upgrade validators.
        let important: Vec<_> = [
            warnings.iter().find(|w| w.code == "ExpansionLimited"),
            warnings.iter().find(|w| w.code == "ExpandedRule"),
            warnings.iter().find(|w| !informational(w)),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        let mut codes = std::collections::BTreeSet::new();
        let important: Vec<_> = important
            .into_iter()
            .filter(|w| codes.insert(w.code.clone()))
            .collect();
        warnings.truncate(LIMIT - 1);
        for w in important {
            if warnings.iter().any(|kept| kept.code == w.code) {
                continue;
            }
            // Never evict the sole kept copy of any important code, even if it
            // occupies an old fixed replacement index (the former index-47 bug).
            let index = warnings
                .iter()
                .rposition(|kept| {
                    !codes.contains(&kept.code)
                        || warnings
                            .iter()
                            .filter(|other| other.code == kept.code)
                            .count()
                            > 1
                })
                .expect("49 slots cannot all be sole copies of three important codes");
            warnings[index] = w;
        }
        warnings.push(Warning {
            uid: None,
            code: "WarningsTruncated".into(),
            message: format!("{} additional warnings omitted", total - warnings.len()),
        });
    }
    for w in &mut warnings {
        // At most 200 characters of text per entry (all fields combined).
        w.code = clean_text(&w.code, 40);
        w.uid = w.uid.as_ref().map(|s| clean_text(s, 80));
        let used = w.code.chars().count() + w.uid.as_ref().map_or(0, |s| s.chars().count());
        w.message = clean_text(&w.message, 200 - used);
    }
    warnings
}

pub async fn run_due(
    pool: &PgPool,
    now: DateTime<Utc>,
    fetcher: &fetch::Fetcher,
) -> anyhow::Result<()> {
    let ids: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "select household_id,id from porchlight.ics_sources where kind='feed' and enabled and deleted_at is null and next_fetch_at<=$1 order by next_fetch_at,id limit 50",
    ).bind(now).fetch_all(pool).await?;
    stream::iter(ids)
        .map(|(household, id)| async move {
            if refresh_source(pool, household, id, now, fetcher)
                .await
                .is_err()
            {
                tracing::warn!(source_id=%id,"calendar source refresh transaction failed");
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    Ok(())
}

pub async fn refresh_source(
    pool: &PgPool,
    household: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
    fetcher: &fetch::Fetcher,
) -> Result<bool, ApiError> {
    // No transaction or source row lock spans DNS/TLS/body reads. Configuration and
    // manual-refresh version changes invalidate this optimistic fetch result.
    let source = sqlx::query_scalar::<_, Json<Source>>(
        "select to_jsonb(s) from porchlight.ics_sources s
         where household_id=$1 and id=$2 and kind='feed' and enabled
           and deleted_at is null and next_fetch_at<=$3",
    )
    .bind(household)
    .bind(id)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    let Some(Json(source)) = source else {
        return Ok(false);
    };
    let active: bool = sqlx::query_scalar(
        "select exists(select 1 from porchlight.household_members
         where household_id=$1 and id=$2 and deleted_at is null)
         and ($3::uuid is null or exists(select 1 from porchlight.household_members
         where household_id=$1 and id=$3 and deleted_at is null))",
    )
    .bind(household)
    .bind(source.owner_member_id)
    .bind(source.target_member_id)
    .fetch_one(pool)
    .await?;
    let fetched = if active {
        Some(
            fetcher
                .fetch(
                    source.url.as_deref().unwrap_or(""),
                    source.allow_http,
                    source
                        .etag
                        .as_deref()
                        .filter(|_| !needs_unconditional_fetch(&source.last_warnings)),
                    source
                        .last_modified
                        .as_deref()
                        .filter(|_| !needs_unconditional_fetch(&source.last_warnings)),
                )
                .await,
        )
    } else {
        None
    };
    let tx = begin_as(pool, &Actor::system()).await?;
    let mut tx = match claim(tx, household, id, now, &source, true).await {
        Ok(tx) => tx,
        Err(done) => return done,
    };
    if !active {
        sqlx::query(
            "update porchlight.ics_sources set enabled=false,last_status='failed',
             last_error='member_removed',last_fetch_at=$2 where id=$1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(true);
    }
    // R80: consecutive apply-deadline failures, whatever bytes were fetched. Success, 304 and
    // fetch/content failures decide below whether it resets (success/304) or is preserved.
    let streak = apply_timeout_streak(&mut tx, id).await?;
    let mut timeout_count = 0_i32;
    let mut status = "failed";
    let mut error = None;
    let mut etag = source.etag.clone();
    let mut modified = source.last_modified.clone();
    let mut warnings = source.last_warnings.clone();
    match fetched.expect("active source fetched") {
        Ok(result) => {
            if let Some(content) = result.content {
                let caller = Caller::Agent {
                    token_id: Uuid::nil(),
                    member_id: source.owner_member_id,
                    label: "calendar scheduler".into(),
                    scopes: vec!["admin".into()],
                    household_id: household,
                    request_id: Uuid::now_v7(),
                };
                let mut domain = tx.begin().await?;
                match apply_calendar(&mut domain, &caller, &source, &content, true, now).await {
                    Ok(summary) => {
                        domain.commit().await?;
                        etag = result.etag;
                        modified = result.last_modified;
                        (status, warnings) =
                            fetch_warning_state(summary.warnings, &mut etag, &mut modified);
                    }
                    Err(e) if e.is_calendar_apply_timeout() => {
                        // R86: the deadline dropped the apply mid-protocol; landing on a
                        // savepoint begin silently rolls `tx` back (claim and actor settings
                        // included) into autocommit. Poisoned either way: discard it and record
                        // the deadline in a fresh attributed transaction that re-claims the
                        // source row (waiting for its lock, not skipping it).
                        drop(domain);
                        drop(tx);
                        let fresh = begin_as(pool, &Actor::system()).await?;
                        tx = match claim(fresh, household, id, now, &source, false).await {
                            Ok(tx) => tx,
                            Err(done) => return done,
                        };
                        timeout_count = (apply_timeout_streak(&mut tx, id).await? + 1).min(3);
                        status = if timeout_count >= 3 {
                            "failed"
                        } else {
                            "retrying"
                        };
                        error = Some(if timeout_count >= 3 {
                            "apply_timeout"
                        } else {
                            "apply_retryable"
                        });
                    }
                    Err(e) => {
                        domain.rollback().await?;
                        if e.is_transient() {
                            // Contention keeps immediate next-poll retry, with no state change.
                            tx.rollback().await?;
                            return Err(e);
                        } else {
                            timeout_count = streak; // non-deadline: neither advances nor resets
                            error = Some(if e.problem() == Problem::CalendarTooLarge {
                                "calendar_too_large"
                            } else {
                                "invalid_calendar"
                            });
                        }
                    }
                }
            } else {
                status = if warning_status(&warnings) == "partial" {
                    "partial"
                } else {
                    "not_modified"
                };
                if result.etag.is_some() {
                    etag = result.etag;
                }
                if result.last_modified.is_some() {
                    modified = result.last_modified;
                }
            }
        }
        Err(e) => {
            timeout_count = streak; // non-deadline: neither advances nor resets
            error = Some(e.0);
        }
    }
    // Default daily interval is a household-local nightly run, not creation+24h.
    // Failed transport/parse backs off 5, 10, 20... minutes, capped at the
    // interval. Success (including partial/304) resets it. Only deadlines back off;
    // three consecutive deadlines (any content) escalate to failed/feed_error.
    let failure_count = if matches!(status, "failed" | "retrying") {
        source.failure_count.saturating_add(1)
    } else {
        0
    };
    let next = if matches!(status, "failed" | "retrying") {
        let seconds = (300_i64 * (1_i64 << (failure_count - 1).min(14)))
            .min(i64::from(source.refresh_seconds));
        now + chrono::Duration::seconds(seconds)
    } else if source.refresh_seconds == 86400 {
        let zone = calendar_zone(&mut tx, household).await?;
        let local = porchlight_core::time::to_local(now, zone);
        let tonight = local.date().and_hms_opt(2, 30, 0).expect("nightly time");
        let candidate = porchlight_core::time::resolve_local(tonight, zone);
        if candidate > now {
            candidate
        } else {
            porchlight_core::time::resolve_local(tonight + chrono::Duration::days(1), zone)
        }
    } else {
        now + chrono::Duration::seconds(i64::from(source.refresh_seconds))
    };
    sqlx::query(
        "update porchlight.ics_sources
         set last_fetch_at=$2,last_status=$3,last_error=$4,etag=$5,last_modified=$6,
             next_fetch_at=$7,failure_count=$8,last_warnings=$9,apply_timeout_count=$10 where id=$1",
    )
    .bind(id)
    .bind(now)
    .bind(status)
    .bind(error)
    .bind(etag)
    .bind(modified)
    .bind(next)
    .bind(failure_count)
    .bind(Json(bounded_warnings(warnings)))
    .bind(timeout_count)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}
/// Locks the due source row for this refresh and rechecks it against the optimistic fetch.
/// Otherwise the transaction is rolled back and the refresh result is returned instead: not
/// due any more (or, with `skip_locked`, held by another refresh), or a transient conflict.
async fn claim(
    mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
    household: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
    source: &Source,
    skip_locked: bool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, Result<bool, ApiError>> {
    let sql = if skip_locked {
        "select to_jsonb(s) from porchlight.ics_sources s
         where household_id=$1 and id=$2 and kind='feed' and enabled
           and deleted_at is null and next_fetch_at<=$3
         for no key update skip locked"
    } else {
        "select to_jsonb(s) from porchlight.ics_sources s
         where household_id=$1 and id=$2 and kind='feed' and enabled
           and deleted_at is null and next_fetch_at<=$3
         for no key update"
    };
    let locked = sqlx::query_scalar::<_, Json<Source>>(sql)
        .bind(household)
        .bind(id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| Err(e.into()))?;
    let done = match locked {
        Some(Json(current)) if current.version == source.version => return Ok(tx),
        Some(_) => Err(ApiError::transient(Problem::Conflict).retry(1)),
        None => Ok(false),
    };
    Err(match tx.rollback().await {
        Ok(()) => done,
        Err(e) => Err(e.into()),
    })
}
async fn apply_timeout_streak(conn: &mut sqlx::PgConnection, id: Uuid) -> Result<i32, ApiError> {
    Ok(
        sqlx::query_scalar("select apply_timeout_count from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(conn)
            .await?,
    )
}
async fn calendar_zone(
    conn: &mut sqlx::PgConnection,
    household: Uuid,
) -> Result<chrono_tz::Tz, ApiError> {
    target_zone(conn, household, None).await
}

#[cfg(test)]
mod fix3_tests {
    use super::*;
    fn warning(code: &str) -> Warning {
        Warning {
            uid: Some("remote\n\u{202e}".repeat(80)),
            code: code.into(),
            message: "remote\u{1b}\u{2066}".repeat(80),
        }
    }
    #[test]
    fn truncation_never_overwrites_a_kept_expansion_signal() {
        let mut warnings = vec![warning("DurationNormalized"); 47];
        warnings.push(warning("ExpansionLimited"));
        warnings.extend(vec![warning("DurationNormalized"); 20]);
        warnings.push(warning("ExpandedRule"));
        let kept = bounded_warnings(warnings);
        assert!(kept.iter().any(|w| w.code == "ExpansionLimited"));
        assert!(kept.iter().any(|w| w.code == "ExpandedRule"));
        assert_eq!(warning_status(&kept), "partial");
    }
    #[test]
    fn expansion_limit_forces_unconditional_retry_and_warning_storage_is_bounded() {
        let mut warnings = vec![warning("InvalidEvent"); 70];
        warnings.push(warning("ExpansionLimited"));
        warnings.push(warning("ExpandedRule"));
        let mut etag = Some("validator".into());
        let mut modified = Some("validator".into());
        let (status, stored) = fetch_warning_state(warnings, &mut etag, &mut modified);
        assert_eq!(status, "partial");
        assert!(etag.is_none() && modified.is_none());
        assert_eq!(stored.len(), 50);
        assert!(stored.iter().any(|w| w.code == "ExpansionLimited"));
        assert!(stored.iter().any(|w| w.code == "ExpandedRule"));
        assert!(stored.iter().any(|w| w.code == "WarningsTruncated"));
        for w in stored {
            let text = format!("{}{}{}", w.code, w.uid.unwrap_or_default(), w.message);
            assert!(text.chars().count() <= 200);
            assert!(
                !text
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '\u{202e}' | '\u{2066}'))
            );
        }
    }
    #[test]
    fn expanded_rule_is_informational_but_forces_unconditional_refetch() {
        let mut etag = Some("validator".into());
        let mut modified = Some("validator".into());
        let (status, warnings) =
            fetch_warning_state(vec![warning("ExpandedRule")], &mut etag, &mut modified);
        assert_eq!(status, "ok");
        assert_eq!(warnings[0].code, "ExpandedRule");
        assert!(etag.is_none() && modified.is_none());
    }
    #[test]
    fn informational_warnings_do_not_make_a_source_partial_even_when_truncated() {
        let mut etag = Some("validator".into());
        let mut modified = Some("validator".into());
        let (status, stored) = fetch_warning_state(
            vec![warning("DurationNormalized"); 80],
            &mut etag,
            &mut modified,
        );
        assert_eq!(status, "ok");
        assert_eq!(warning_status(&stored), "ok");
        assert!(etag.is_some() && modified.is_some());
    }
}
