//! Single route registry builds every trust-boundary router and drives coverage/documentation tests.
pub mod admin;
pub mod administration;
pub mod auth;
pub mod backups;
pub mod calendar;
pub mod collaboration;
pub mod connection;
pub mod devices;
pub mod enrollment;
pub mod exports;
pub mod items;
pub mod lists;
pub mod me;
pub mod members;
pub mod notifications;
pub mod planning;
pub mod read_models;
pub mod reads;
pub mod release;
pub mod stream;
use crate::{
    error::{ApiError, Problem},
    state::AppState,
};
use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Mtls,
    Enrollment,
    Agent,
}
#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub surface: Surface,
    pub area: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub handler: &'static str,
    pub legacy: bool,
    pub terminal: Terminal,
}
/// Terminal exemptions belong to registration, not to hard-coded URI strings.
#[derive(Clone, Copy, Debug)]
pub enum Terminal {
    None,
    Always,
    DeviceTarget,
    PatTarget,
}
macro_rules! method {
    (UPLOAD, $handler:path) => {
        crate::mutation::multipart($handler)
    };
    (GET, $handler:path) => {
        axum::routing::get($handler)
    };
    (LEGACY_POST, $handler:path) => {
        crate::mutation::legacy_post($handler)
    };
    (POST, $handler:path) => {
        crate::mutation::post($handler)
    };
    (PUT, $handler:path) => {
        crate::mutation::put($handler)
    };
    (PATCH, $handler:path) => {
        crate::mutation::patch($handler)
    };
    (DELETE, $handler:path) => {
        crate::mutation::delete($handler)
    };
}
macro_rules! verb {
    (UPLOAD) => {
        "POST"
    };
    (LEGACY_POST) => {
        "POST"
    };
    ($method:ident) => {
        stringify!($method)
    };
}
macro_rules! legacy {
    (LEGACY_POST) => {
        true
    };
    ($method:ident) => {
        false
    };
}
macro_rules! body_policy {
    (calendar,import) => {
        crate::extract::BodyPolicy::Calendar
    };
    ($area:ident,$handler:ident) => {
        crate::extract::BodyPolicy::Json
    };
}
// OpenAPI path parameters do not include axum's catch-all marker (the registry test maps through this too).
pub fn mount_path(path: &str) -> &str {
    match path {
        "/v1/client-release/files/{path}" => "/v1/client-release/files/{*path}",
        _ => path,
    }
}
macro_rules! registry {
    ($(($surface:ident,$area:ident,$method:ident,$path:literal,$handler:ident,$terminal:ident)),* $(,)?) => {
        pub const ROUTES: &[Route] = &[$(Route {surface:Surface::$surface,area:stringify!($area),method:verb!($method),path:$path,handler:stringify!($handler),legacy:legacy!($method),terminal:Terminal::$terminal}),*];
        // The final routers are built here directly; areas cannot append unregistered routes.
        pub fn registry_router(surface:Surface) -> Router<AppState> {
            let mut router=base();
            $(if surface==Surface::$surface {
                router=router.route(mount_path($path),method!($method,$area::$handler).layer::<_,std::convert::Infallible>(axum::Extension(Terminal::$terminal)).layer::<_,std::convert::Infallible>(axum::Extension(body_policy!($area,$handler))));
            })*
            router
        }
    }
}
registry! {
    (Mtls,release,GET,"/v1/client-release",client,None),
    (Mtls,release,GET,"/v1/client-release/files/{path}",download,None),
    (Mtls,release,GET,"/v1/server-release",server,None),
    (Agent,release,GET,"/v1/admin/release",admin_status,None),
    (Mtls,backups,GET,"/v1/backups",status,None),
    (Mtls,backups,GET,"/v1/backups/latest",latest,None),
    (Mtls,backups,GET,"/v1/backups/latest/download",download,None),
    (Agent,backups,GET,"/v1/admin/backups",admin_status,None),
    (Mtls,exports,GET,"/v1/export/member",member,None),
    (Agent,exports,GET,"/v1/export/member",member,None),
    (Mtls,exports,GET,"/v1/admin/export/household",household,None),
    (Agent,exports,GET,"/v1/admin/export/household",household,None),
    (Agent,administration,GET,"/v1/admin/users",users,None),
    (Agent,administration,POST,"/v1/admin/users",add_user,None),
    (Agent,administration,PATCH,"/v1/admin/users/{id}",edit_user,None),
    (Agent,administration,DELETE,"/v1/admin/users/{id}",delete_user,None),
    (Agent,administration,POST,"/v1/admin/items/{id}/revert",revert,None),
    (Agent,administration,GET,"/v1/admin/items/{id}/revisions",revisions,None),
    (Agent,administration,POST,"/v1/admin/purge",purge,None),
    (Agent,administration,GET,"/v1/admin/derive",derive_status,None),
    (Agent,administration,POST,"/v1/admin/derive/{id}/retry",derive_retry,None),
    (Mtls,notifications,GET,"/v1/notifications",list,None),
    (Mtls,notifications,POST,"/v1/notifications/{id}/read",read,None),
    (Mtls,notifications,POST,"/v1/notifications/read-all",read_all,None),
    (Mtls,notifications,GET,"/v1/me/notification-prefs",prefs_get,None),
    (Mtls,notifications,PUT,"/v1/me/notification-prefs",prefs_put,None),
    (Agent,notifications,GET,"/v1/notifications",list,None),
    (Agent,notifications,POST,"/v1/notifications/{id}/read",read,None),
    (Agent,notifications,POST,"/v1/notifications/read-all",read_all,None),
    (Agent,notifications,GET,"/v1/me/notification-prefs",prefs_get,None),
    (Agent,notifications,PUT,"/v1/me/notification-prefs",prefs_put,None),
    (Agent,notifications,GET,"/v1/admin/notifications",admin_list,None),
    (Mtls,collaboration,GET,"/v1/items/{id}/comments",list_comments,None),
    (Agent,collaboration,GET,"/v1/items/{id}/comments",list_comments,None),
    (Mtls,collaboration,POST,"/v1/items/{id}/comments",create_comment,None),
    (Agent,collaboration,POST,"/v1/items/{id}/comments",create_comment,None),
    (Mtls,collaboration,PATCH,"/v1/comments/{id}",patch_comment,None),
    (Agent,collaboration,PATCH,"/v1/comments/{id}",patch_comment,None),
    (Mtls,collaboration,DELETE,"/v1/comments/{id}",delete_comment,None),
    (Agent,collaboration,DELETE,"/v1/comments/{id}",delete_comment,None),
    (Mtls,collaboration,UPLOAD,"/v1/items/{id}/attachments",upload_item,None),
    (Agent,collaboration,UPLOAD,"/v1/items/{id}/attachments",upload_item,None),
    (Mtls,collaboration,UPLOAD,"/v1/comments/{id}/attachments",upload_comment,None),
    (Agent,collaboration,UPLOAD,"/v1/comments/{id}/attachments",upload_comment,None),
    (Mtls,collaboration,GET,"/v1/attachments/{id}",download,None),
    (Agent,collaboration,GET,"/v1/attachments/{id}",download,None),
    (Mtls,collaboration,GET,"/v1/attachments/{id}/thumbnail",thumbnail,None),
    (Agent,collaboration,GET,"/v1/attachments/{id}/thumbnail",thumbnail,None),
    (Mtls,collaboration,DELETE,"/v1/attachments/{id}",delete_attachment,None),
    (Agent,collaboration,DELETE,"/v1/attachments/{id}",delete_attachment,None),
    (Mtls,calendar,POST,"/v1/imports/ics",import,None),
    (Agent,calendar,POST,"/v1/imports/ics",import,None),
    (Mtls,calendar,GET,"/v1/feeds",list,None),
    (Mtls,calendar,POST,"/v1/feeds",add,None),
    (Mtls,calendar,PUT,"/v1/feeds/{id}",update,None),
    (Mtls,calendar,DELETE,"/v1/feeds/{id}",remove,None),
    (Mtls,calendar,POST,"/v1/feeds/{id}/refresh",refresh,None),
    (Agent,calendar,GET,"/v1/admin/feeds",admin_list,None),
    (Agent,calendar,GET,"/v1/admin/feeds/target",resolve_target,None),
    (Agent,calendar,POST,"/v1/admin/feeds",admin_add,None),
    (Agent,calendar,PUT,"/v1/admin/feeds/{id}",admin_update,None),
    (Agent,calendar,DELETE,"/v1/admin/feeds/{id}",admin_remove,None),
    (Agent,calendar,POST,"/v1/admin/feeds/{id}/refresh",admin_refresh,None),
    // s8b: planning, dual authenticated surfaces.
    (Mtls,planning,GET,"/v1/members/{id}/availability",availability,None),
    (Agent,planning,GET,"/v1/members/{id}/availability",availability,None),
    (Mtls,planning,PUT,"/v1/members/{id}/availability",put_availability,None),
    (Agent,planning,PUT,"/v1/members/{id}/availability",put_availability,None),
    (Mtls,planning,POST,"/v1/schedule/suggest",suggest,None),
    (Agent,planning,POST,"/v1/schedule/suggest",suggest,None),
    (Mtls,planning,GET,"/v1/suggestions",suggestions,None),
    (Agent,planning,GET,"/v1/suggestions",suggestions,None),
    (Mtls,planning,POST,"/v1/suggestions/{id}/accept",accept,None),
    (Agent,planning,POST,"/v1/suggestions/{id}/accept",accept,None),
    (Mtls,planning,POST,"/v1/suggestions/{id}/dismiss",dismiss,None),
    (Agent,planning,POST,"/v1/suggestions/{id}/dismiss",dismiss,None),
    (Mtls,planning,GET,"/v1/items/{id}/candidates",candidates,None),
    (Agent,planning,GET,"/v1/items/{id}/candidates",candidates,None),
    (Mtls,lists,GET,"/v1/lists",list,None),
    (Agent,lists,GET,"/v1/lists",list,None),
    (Mtls,lists,POST,"/v1/lists",create,None),
    (Agent,lists,POST,"/v1/lists",create,None),
    (Mtls,lists,GET,"/v1/lists/{id}",get,None),
    (Agent,lists,GET,"/v1/lists/{id}",get,None),
    (Mtls,lists,PATCH,"/v1/lists/{id}",patch,None),
    (Agent,lists,PATCH,"/v1/lists/{id}",patch,None),
    (Mtls,lists,DELETE,"/v1/lists/{id}",delete,None),
    (Agent,lists,DELETE,"/v1/lists/{id}",delete,None),
    (Mtls,lists,POST,"/v1/lists/{id}/entries",add_entry,None),
    (Agent,lists,POST,"/v1/lists/{id}/entries",add_entry,None),
    (Mtls,lists,PATCH,"/v1/list-entries/{id}",patch_entry,None),
    (Agent,lists,PATCH,"/v1/list-entries/{id}",patch_entry,None),
    (Mtls,lists,DELETE,"/v1/list-entries/{id}",delete_entry,None),
    (Agent,lists,DELETE,"/v1/list-entries/{id}",delete_entry,None),
    (Mtls,lists,POST,"/v1/list-entries/{id}/check",check,None),
    (Agent,lists,POST,"/v1/list-entries/{id}/check",check,None),
    (Mtls,lists,POST,"/v1/list-entries/{id}/uncheck",uncheck,None),
    (Agent,lists,POST,"/v1/list-entries/{id}/uncheck",uncheck,None),
    (Mtls,lists,POST,"/v1/lists/{id}/clear-checked",clear,None),
    (Agent,lists,POST,"/v1/lists/{id}/clear-checked",clear,None),
    (Mtls,lists,PUT,"/v1/lists/{id}/viewers",viewers,None),
    (Agent,lists,PUT,"/v1/lists/{id}/viewers",viewers,None),
    (Mtls,reads,GET,"/v1/board",board,None),
    (Mtls,reads,GET,"/v1/agenda",agenda,None),
    (Mtls,reads,GET,"/v1/snapshot",snapshot,None),
    (Mtls,reads,GET,"/v1/search",search,None),
    (Mtls,members,GET,"/v1/members",list,None),
    (Mtls,members,PATCH,"/v1/members/{id}",patch,None),
    (Mtls,members,GET,"/v1/me/settings",settings_get,None),
    (Mtls,members,PUT,"/v1/me/settings",settings_put,None),
    (Agent,reads,GET,"/v1/board",board,None),
    (Agent,reads,GET,"/v1/agenda",agenda,None),
    (Agent,reads,GET,"/v1/snapshot",snapshot,None),
    (Agent,reads,GET,"/v1/search",search,None),
    (Agent,members,GET,"/v1/members",list,None),
    (Agent,members,PATCH,"/v1/members/{id}",patch,None),
    (Agent,members,GET,"/v1/me/settings",settings_get,None),
    (Agent,members,PUT,"/v1/me/settings",settings_put,None),
    (Mtls,items,GET,"/v1/items",list,None),
    (Agent,items,GET,"/v1/items",list,None),
    (Mtls,items,POST,"/v1/items",create,None),
    (Agent,items,POST,"/v1/items",create,None),
    (Mtls,items,GET,"/v1/items/{id}",get,None),
    (Agent,items,GET,"/v1/items/{id}",get,None),
    (Mtls,items,PATCH,"/v1/items/{id}",patch,None),
    (Agent,items,PATCH,"/v1/items/{id}",patch,None),
    (Mtls,items,DELETE,"/v1/items/{id}",delete,None),
    (Agent,items,DELETE,"/v1/items/{id}",delete,None),
    (Mtls,items,PUT,"/v1/items/{id}/assignees",assignees,None),
    (Agent,items,PUT,"/v1/items/{id}/assignees",assignees,None),
    (Mtls,items,PUT,"/v1/items/{id}/viewers",viewers,None),
    (Agent,items,PUT,"/v1/items/{id}/viewers",viewers,None),
    (Mtls,items,PUT,"/v1/items/{id}/reminders",reminders,None),
    (Agent,items,PUT,"/v1/items/{id}/reminders",reminders,None),
    (Mtls,items,POST,"/v1/items/{id}/take",take,None),
    (Agent,items,POST,"/v1/items/{id}/take",take,None),
    (Mtls,items,GET,"/v1/items/{id}/occurrences",occurrences,None),
    (Agent,items,GET,"/v1/items/{id}/occurrences",occurrences,None),
    (Mtls,items,PATCH,"/v1/occurrences/{id}",patch_occurrence,None),
    (Agent,items,PATCH,"/v1/occurrences/{id}",patch_occurrence,None),
    (Mtls,items,POST,"/v1/occurrences/{id}/complete",complete,None),
    (Agent,items,POST,"/v1/occurrences/{id}/complete",complete,None),
    (Mtls,items,POST,"/v1/occurrences/{id}/reopen",reopen,None),
    (Agent,items,POST,"/v1/occurrences/{id}/reopen",reopen,None),
    (Mtls,connection,GET,"/v1/connection",get,None),
    (Mtls,me,GET,"/v1/me",get,None),
    (Mtls,stream,GET,"/v1/stream",get,None),
    (Mtls,auth,LEGACY_POST,"/v1/auth/login",login,None),
    (Mtls,auth,LEGACY_POST,"/v1/auth/logout",logout,Always),
    (Mtls,auth,LEGACY_POST,"/v1/auth/password",password,None),
    (Mtls,auth,LEGACY_POST,"/v1/auth/reauth",reauth,None),
    (Mtls,auth,LEGACY_POST,"/v1/auth/reset-password",reset_password,None),
    (Mtls,devices,LEGACY_POST,"/v1/devices/self/certificate",certificate,None),
    (Mtls,devices,GET,"/v1/devices",list,None),
    (Mtls,devices,LEGACY_POST,"/v1/devices/{id}/revoke",revoke,DeviceTarget),
    (Mtls,devices,GET,"/v1/endpoints",endpoints,None),
    (Enrollment,enrollment,LEGACY_POST,"/v1/enrollment/complete",complete,None),
    (Agent,admin,GET,"/v1/admin/whoami",whoami,None),
    (Agent,admin,LEGACY_POST,"/v1/admin/enrollment-tokens",enrollment_tokens,None),
    (Agent,admin,LEGACY_POST,"/v1/admin/users/{username}/password-reset",password_reset,None),
    (Agent,admin,GET,"/v1/admin/devices",list_devices,None),
    (Agent,admin,LEGACY_POST,"/v1/admin/devices/{id}/revoke",revoke_device,None),
    (Agent,admin,LEGACY_POST,"/v1/admin/tokens",create_token,None),
    (Agent,admin,LEGACY_POST,"/v1/admin/tokens/{id}/revoke",revoke_token,PatTarget),
}
async fn enroll_limit(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if matches!(
        *req.method(),
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::PATCH
            | axum::http::Method::DELETE
    ) {
        let addr = req
            .extensions()
            .get::<crate::server::ConnInfo>()
            .and_then(|i| i.client_addr)
            .map(|a| a.ip());
        state
            .limits
            .check(crate::ratelimit::Class::Auth, None, addr)?;
    }
    Ok(next.run(req).await)
}
fn base() -> Router<AppState> {
    Router::new().fallback(|| async { ApiError::new(Problem::NotFound) })
}
fn finish(router: Router<AppState>, state: AppState) -> Router {
    router
        .method_not_allowed_fallback(|| async { ApiError::new(Problem::MethodNotAllowed) })
        .layer(middleware::from_fn(crate::mutation::require_key))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::protocol::request_headers,
        ))
        .with_state(state)
}
pub fn mtls_router(state: AppState) -> Router {
    finish(registry_router(Surface::Mtls), state)
}
pub fn enroll_router(state: AppState) -> Router {
    finish(
        registry_router(Surface::Enrollment)
            .layer(middleware::from_fn_with_state(state.clone(), enroll_limit)),
        state,
    )
}
pub fn agent_router(state: AppState) -> Router {
    finish(registry_router(Surface::Agent), state)
}

// Test-only registry exercises the same macro and constructors as domain areas.
#[cfg(test)]
pub(crate) mod test_registry {
    use super::*;
    pub mod dual {
        use super::*;
        #[utoipa::path(get, path="/test/dual", responses((status=200,description="Read"),crate::openapi::UniversalProblems), security(("session"=[])))]
        pub async fn read() -> &'static str {
            "read"
        }
        #[utoipa::path(post, path="/test/dual", request_body=serde_json::Value, responses((status=204,description="Created"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems), security(("session"=[])))]
        pub fn create<'a>(
            _: &'a AppState,
            _: crate::extract::Caller,
            _: &'a mut crate::idempotency::IdemTx,
            _: serde_json::Value,
        ) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
            Box::pin(async {
                Ok(axum::response::IntoResponse::into_response(
                    axum::http::StatusCode::NO_CONTENT,
                ))
            })
        }
        #[derive(utoipa::OpenApi)]
        #[openapi(paths(read, create))]
        pub struct DualApi;
    }
    registry! {
        (Mtls,dual,GET,"/test/dual",read,None),
        (Agent,dual,GET,"/test/dual",read,None),
        (Mtls,dual,POST,"/test/dual",create,None),
        (Agent,dual,POST,"/test/dual",create,None),
    }
    #[test]
    fn both_surfaces_mount_the_shared_path_and_methods() {
        for surface in [Surface::Mtls, Surface::Agent] {
            let debug = format!("{:?}", registry_router(surface));
            assert!(debug.contains("\"/test/dual\""));
            for method in ["get", "post"] {
                assert!(
                    !debug
                        .split(&format!("{method}: "))
                        .nth(1)
                        .unwrap()
                        .starts_with("None")
                );
            }
            let registered: std::collections::BTreeSet<_> = ROUTES
                .iter()
                .filter(|r| r.surface == surface)
                .map(|r| (r.path, r.method))
                .collect();
            assert_eq!(registered.len(), 2);
        }
    }
}
