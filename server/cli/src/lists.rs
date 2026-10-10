//! Lists over the agent socket. Explicit versions preserve normal API conflicts.
use crate::client::{Client, Error, Result};
use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Persistent,
    PerOccurrence,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Visibility {
    Household,
    Private,
    Selected,
}
#[derive(Subcommand)]
pub enum ListsCommand {
    List {
        #[arg(long)]
        item_id: Option<Uuid>,
        /// Include deleted-parent lists for owner/admin cleanup; use rm --version from this page.
        #[arg(long)]
        include_orphaned: bool,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u16>,
    },
    /// Show by UUID or unambiguous name (case insensitive).
    Show {
        id: String,
        #[arg(long)]
        occurrence_id: Option<Uuid>,
    },
    Add {
        name: String,
        #[arg(long)]
        item_id: Option<Uuid>,
        #[arg(long)]
        owner_id: Option<Uuid>,
        #[arg(long, value_enum, default_value = "persistent")]
        mode: Mode,
        #[arg(long, value_enum, default_value = "household")]
        visibility: Visibility,
        #[arg(long, value_delimiter = ',')]
        viewers: Vec<Uuid>,
    },
    AddEntry {
        list_id: String,
        label: String,
        #[arg(long)]
        qty: Option<f64>,
        #[arg(long, default_value = "")]
        unit: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "")]
        notes: String,
        #[arg(long)]
        after_id: Option<Uuid>,
        #[arg(long)]
        before_id: Option<Uuid>,
    },
    Check {
        id: Uuid,
        #[arg(long)]
        list_id: Option<Uuid>,
        #[arg(long)]
        occurrence_id: Option<Uuid>,
        #[arg(long)]
        version: Option<i32>,
    },
    Uncheck {
        id: Uuid,
        #[arg(long)]
        list_id: Option<Uuid>,
        #[arg(long)]
        occurrence_id: Option<Uuid>,
        #[arg(long)]
        version: Option<i32>,
    },
    Clear {
        id: String,
        #[arg(long)]
        version: Option<i32>,
    },
    Rm {
        id: String,
        #[arg(long)]
        version: Option<i32>,
    },
    /// Edit standalone metadata; entry rights do not grant this action.
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        owner_id: Option<Uuid>,
        #[arg(long, value_enum)]
        visibility: Option<Visibility>,
        #[arg(long)]
        version: Option<i32>,
    },
    EditEntry {
        id: Uuid,
        #[arg(long)]
        list_id: Uuid,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, conflicts_with = "clear_qty")]
        qty: Option<f64>,
        #[arg(long)]
        clear_qty: bool,
        #[arg(long)]
        unit: Option<String>,
        #[arg(long, conflicts_with = "clear_url")]
        url: Option<String>,
        #[arg(long)]
        clear_url: bool,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        after_id: Option<Uuid>,
        #[arg(long)]
        before_id: Option<Uuid>,
        #[arg(long)]
        version: Option<i32>,
    },
    RmEntry {
        id: Uuid,
        #[arg(long)]
        list_id: Uuid,
        #[arg(long)]
        version: Option<i32>,
    },
    Viewers {
        id: String,
        #[arg(value_delimiter = ',')]
        viewers: Vec<Uuid>,
        #[arg(long)]
        version: Option<i32>,
    },
}
fn visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Household => "household",
        Visibility::Private => "private",
        Visibility::Selected => "selected",
    }
}
async fn resolve(c: &Client, reference: &str) -> Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return Ok(id);
    }
    let mut path = "/v1/lists?limit=200".to_string();
    let mut found = None;
    loop {
        let page = c.request("GET", &path, Value::Null).await?;
        for list in page["items"]
            .as_array()
            .ok_or(Error::Local("Invalid list collection"))?
        {
            if list["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(reference))
            {
                if found.is_some() {
                    return Err(Error::Local("Ambiguous list name; use UUID"));
                }
                found = Some(
                    Uuid::parse_str(list["id"].as_str().ok_or(Error::Local("Invalid list id"))?)
                        .map_err(|_| Error::Local("Invalid list id"))?,
                );
            }
        }
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        path = format!(
            "/v1/lists?limit=200&cursor={}",
            crate::client::segment(cursor)
        );
    }
    found.ok_or(Error::Local("List not found"))
}
async fn list_version(c: &Client, id: Uuid, version: Option<i32>) -> Result<i32> {
    if let Some(v) = version {
        return Ok(v);
    }
    let r = c
        .request("GET", &format!("/v1/lists/{id}"), Value::Null)
        .await?;
    version_of(&r)
}
fn version_of(r: &Value) -> Result<i32> {
    r["version"]
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or(Error::Local("Invalid version"))
}
async fn entry_version(
    c: &Client,
    id: Uuid,
    list_id: Option<Uuid>,
    version: Option<i32>,
) -> Result<i32> {
    if let Some(v) = version {
        return Ok(v);
    }
    let mut path = list_id
        .map(|id| format!("/v1/lists/{id}"))
        .unwrap_or("/v1/lists?limit=200".into());
    loop {
        let r = c.request("GET", &path, Value::Null).await?;
        let lists = if list_id.is_some() {
            vec![&r]
        } else {
            r["items"]
                .as_array()
                .ok_or(Error::Local("Invalid list collection"))?
                .iter()
                .collect()
        };
        for list in lists {
            for group in ["entries", "got_it"] {
                for entry in list[group]
                    .as_array()
                    .ok_or(Error::Local("Invalid entries"))?
                {
                    if entry["id"].as_str() == Some(&id.to_string()) {
                        return version_of(entry);
                    }
                }
            }
        }
        let Some(cursor) = r["next_cursor"].as_str() else {
            return Err(Error::Local("Entry not found"));
        };
        path = format!(
            "/v1/lists?limit=200&cursor={}",
            crate::client::segment(cursor)
        );
    }
}
fn put(body: &mut Value, key: &str, value: Value) {
    if !value.is_null() {
        body[key] = value;
    }
}
pub async fn dispatch(c: &Client, cmd: &ListsCommand) -> Result<Value> {
    match cmd {
        ListsCommand::List {item_id,cursor,limit,include_orphaned} => {
            let mut p=vec![];
            if *include_orphaned {p.push("include_orphaned=true".into());}
            if let Some(id)=item_id {p.push(format!("item_id={id}"));}
            if let Some(limit)=limit {p.push(format!("limit={limit}"));}
            if let Some(cursor)=cursor {p.push(format!("cursor={}",crate::client::segment(cursor)));}
            let suffix=if p.is_empty() {String::new()} else {format!("?{}",p.join("&"))};
            c.request("GET",&format!("/v1/lists{suffix}"),Value::Null).await
        }
        ListsCommand::Show {id,occurrence_id} => {
            let id=resolve(c,id).await?;
            let suffix=occurrence_id.map(|id|format!("?occurrence_id={id}")).unwrap_or_default();
            c.request("GET",&format!("/v1/lists/{id}{suffix}"),Value::Null).await
        }
        ListsCommand::Add {name,item_id,owner_id,mode,visibility:v,viewers} => {
            c.request("POST","/v1/lists",json!({"name":name,"item_id":item_id,"owner_id":owner_id,"mode":match mode {Mode::Persistent=>"persistent",Mode::PerOccurrence=>"per_occurrence"},"visibility":visibility(*v),"viewers":viewers})).await
        }
        ListsCommand::AddEntry {list_id,label,qty,unit,url,notes,after_id,before_id} => {
            let id=resolve(c,list_id).await?;
            c.request("POST",&format!("/v1/lists/{id}/entries"),json!({"label":label,"qty":qty,"unit":unit,"url":url,"notes":notes,"after_id":after_id,"before_id":before_id})).await
        }
        ListsCommand::Check {id,list_id,occurrence_id,version} | ListsCommand::Uncheck {id,list_id,occurrence_id,version} => {
            let v=entry_version(c,*id,*list_id,*version).await?;
            let action=if matches!(cmd,ListsCommand::Check{..}) {"check"} else {"uncheck"};
            c.request_version("POST",&format!("/v1/list-entries/{id}/{action}"),json!({"occurrence_id":occurrence_id}),Some(i64::from(v))).await
        }
        ListsCommand::Clear {id,version} | ListsCommand::Rm {id,version} => {
            let id=resolve(c,id).await?; let v=list_version(c,id,*version).await?;
            if matches!(cmd,ListsCommand::Clear{..}) {
                c.request_version("POST",&format!("/v1/lists/{id}/clear-checked"),json!({}),Some(i64::from(v))).await
            } else { c.request_version("DELETE",&format!("/v1/lists/{id}"),json!({}),Some(i64::from(v))).await }
        }
        ListsCommand::Edit {id,name,owner_id,visibility:v,version} => {
            let id=resolve(c,id).await?; let version=list_version(c,id,*version).await?;
            let mut body=json!({}); put(&mut body,"name",json!(name)); put(&mut body,"owner_id",json!(owner_id)); put(&mut body,"visibility",json!(v.map(visibility)));
            c.request_version("PATCH",&format!("/v1/lists/{id}"),body,Some(i64::from(version))).await
        }
        ListsCommand::EditEntry {id,list_id,label,qty,clear_qty,unit,url,clear_url,notes,after_id,before_id,version} => {
            let v=entry_version(c,*id,Some(*list_id),*version).await?; let mut body=json!({});
            for (key,value) in [("label",json!(label)),("qty",json!(qty)),("unit",json!(unit)),("url",json!(url)),("notes",json!(notes))] {put(&mut body,key,value);}
            if *clear_qty {body["qty"]=Value::Null;}
            if *clear_url {body["url"]=Value::Null;}
            if after_id.is_some() || before_id.is_some() {body["after_id"]=json!(after_id);body["before_id"]=json!(before_id);}
            c.request_version("PATCH",&format!("/v1/list-entries/{id}"),body,Some(i64::from(v))).await
        }
        ListsCommand::RmEntry {id,list_id,version} => {
            let v=entry_version(c,*id,Some(*list_id),*version).await?;
            c.request_version("DELETE",&format!("/v1/list-entries/{id}"),json!({}),Some(i64::from(v))).await
        }
        ListsCommand::Viewers {id,viewers,version} => {
            let id=resolve(c,id).await?;let v=list_version(c,id,*version).await?;
            c.request_version("PUT",&format!("/v1/lists/{id}/viewers"),json!({"viewers":viewers}),Some(i64::from(v))).await
        }
    }
}
pub fn print(v: &Value) {
    // Human pretty JSON is deliberately escaped, never raw terminal control characters.
    println!("{}", serde_json::to_string_pretty(v).expect("JSON value"));
}
