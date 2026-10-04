#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use dever_core::source::SourceMap;
use dever_runtime::config::RuntimeProfile;
use dever_runtime::orm::{Row, Value};

const MODEL: &str = r#"type Status {
  Active
  Disabled
}

type User {
  uuid: Uuid generated unique
  email: Text(254) unique
  display_name: Text(1, 64)
  status: Status default Status.Active index
  age: Int default 0
  alias: Text?
}

seed {
  {
    email = "seed@example.com"
    display_name = "Seed User"
    age = 10
  }
}
"#;

const APP: &str = r#"show(user: app.user.model.User) () {
  blocking(dever.io.println(user.email))
}

transaction create_transaction_user() (saved: app.user.model.User) {
  created = model.create({
    email = "transaction@example.com"
    display_name = "Transaction User"
    age = 40
  })
  changed = model.update(created.id, {
    alias = "committed"
  })
  saved = model.get(created.id)
}

transaction insert_then_fail() () {
  inserted = model.create({
    email = "rollback@example.com"
    display_name = "Rollback User"
  })
  duplicate = model.create({
    email = "seed@example.com"
    display_name = "Duplicate Seed"
  })
}

main() () {
  transaction_user = create_transaction_user()
  blocking(dever.io.println(transaction_user.display_name))

  created = model.create({
    email = "one@example.com"
    display_name = "One"
    age = 20
  })
  changed = model.update(created.id, {
    display_name = "Updated One"
  })
  loaded = model.get(created.id)
  blocking(dever.io.println(loaded.display_name))

  inserted = model.create_many([
    {
      email = "two@example.com"
      display_name = "Two"
      age = 30
    },
    {
      email = "three@example.com"
      display_name = "Three"
      age = 30
    }
  ])
  blocking(dever.io.println(int.to_text(inserted)))

  saved = model.upsert(
    { email = "two@example.com" },
    {
      display_name = "Ignored"
      age = 30
    },
    { display_name = "Upserted Two" }
  )
  blocking(dever.io.println(saved.display_name))

  total = model.count({
    where = not contains(display_name, "missing") and in(status, [app.user.model.Status.Active]) and between(age, 18, 50) and starts_with(email, "t") and ends_with(email, ".com") and alias == null
  })
  blocking(dever.io.println(int.to_text(total)))

  page = model.list({
    order = id.asc
    page = 1
    size = 2
  })
  blocking(dever.io.println(int.to_text(page.total)))

  cursor = model.cursor({
    order = [age.desc, id.desc]
    size = 2
  })
  blocking(dever.io.println(int.to_text(length(cursor.items))))
  each(show, cursor.items)
  next = model.cursor({
    after = cursor.next
    order = [age.desc, id.desc]
    size = 2
  })
  blocking(dever.io.println(int.to_text(length(next.items))))
  each(show, next.items)

  users = model.stream({
    order = id.asc
    size = 1
  })
  each(show, users)

  removed = model.delete(created.id)
  blocking(dever.io.println(int.to_text(removed)))
}

rollback() () {
  insert_then_fail()
}
"#;

pub const MAIN_STDOUT: &str = concat!(
    "Transaction User\n",
    "Updated One\n",
    "2\n",
    "Upserted Two\n",
    "2\n",
    "5\n",
    "2\n",
    "transaction@example.com\n",
    "three@example.com\n",
    "2\n",
    "two@example.com\n",
    "one@example.com\n",
    "seed@example.com\n",
    "transaction@example.com\n",
    "one@example.com\n",
    "two@example.com\n",
    "three@example.com\n",
    "1\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revisions {
    pub schema: String,
    pub seed: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct UserRow {
    pub email: String,
    pub display_name: String,
    pub status: String,
    pub age: i64,
    pub alias: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DatabaseState {
    pub columns: Vec<String>,
    pub managed_index_count: usize,
    pub schema_revision: String,
    pub seed_revision: String,
    pub users: Vec<UserRow>,
}

pub struct CompiledFixture {
    pub native: dever_core::native::NativeProgram,
    pub revisions: Revisions,
}

pub fn compile(entry: &str, profile: RuntimeProfile) -> CompiledFixture {
    let sources = sources();
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let revisions = revisions(&program.model_snapshot());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let native =
        dever_core::native::compile_application(&program, &sources, entry, &rustc, profile)
            .unwrap();
    CompiledFixture { native, revisions }
}

pub fn configure(executable: &Path, setting: &str) {
    let root = executable.parent().unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/setting.json"), setting).unwrap();
}

pub fn run(executable: &Path) -> Output {
    Command::new(executable).output().unwrap()
}

pub fn assert_main_output(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), MAIN_STDOUT);
    assert!(output.stderr.is_empty());
}

pub fn assert_committed_state(state: &DatabaseState, revisions: &Revisions) {
    assert_eq!(
        state.columns,
        [
            "id",
            "created_at",
            "uuid",
            "email",
            "display_name",
            "status",
            "age",
            "alias"
        ]
    );
    assert_eq!(state.managed_index_count, 3);
    assert_eq!(state.schema_revision, revisions.schema);
    assert_eq!(state.seed_revision, revisions.seed);
    assert_eq!(
        state.users,
        [
            UserRow {
                email: "seed@example.com".into(),
                display_name: "Seed User".into(),
                status: "Active".into(),
                age: 10,
                alias: None,
            },
            UserRow {
                email: "three@example.com".into(),
                display_name: "Three".into(),
                status: "Active".into(),
                age: 30,
                alias: None,
            },
            UserRow {
                email: "transaction@example.com".into(),
                display_name: "Transaction User".into(),
                status: "Active".into(),
                age: 40,
                alias: Some("committed".into()),
            },
            UserRow {
                email: "two@example.com".into(),
                display_name: "Upserted Two".into(),
                status: "Active".into(),
                age: 30,
                alias: None,
            },
        ]
    );
}

pub fn database_state(
    columns: Vec<Row>,
    managed_index_count: Vec<Row>,
    history: Vec<Row>,
    users: Vec<Row>,
) -> DatabaseState {
    DatabaseState {
        columns: columns.iter().map(|row| text(row, 0)).collect(),
        managed_index_count: integer(&managed_index_count[0], 0) as usize,
        schema_revision: text(&history[0], 1),
        seed_revision: text(&history[1], 1),
        users: users
            .iter()
            .map(|row| UserRow {
                email: text(row, 0),
                display_name: text(row, 1),
                status: text(row, 2),
                age: integer(row, 3),
                alias: nullable_text(row, 4),
            })
            .collect(),
    }
}

pub fn sources() -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("app/user/model.dever", MODEL);
    sources.add("app/user/app.dever", APP);
    sources
}

fn revisions(snapshot: &str) -> Revisions {
    let model = snapshot
        .lines()
        .find(|line| line.starts_with("model app.user.model "))
        .expect("shared ORM fixture Model snapshot");
    Revisions {
        schema: snapshot_value(model, "revision"),
        seed: snapshot_value(model, "seed"),
    }
}

fn snapshot_value(line: &str, name: &str) -> String {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("missing {name} in Model snapshot: {line}"))
        .to_owned()
}

fn text(row: &Row, index: usize) -> String {
    match row.get(index).unwrap() {
        Value::Text(value) => value.clone(),
        value => panic!("expected Text at column {index}, got {value:?}"),
    }
}

fn integer(row: &Row, index: usize) -> i64 {
    match row.get(index).unwrap() {
        Value::Int(value) => *value,
        value => panic!("expected Int at column {index}, got {value:?}"),
    }
}

fn nullable_text(row: &Row, index: usize) -> Option<String> {
    match row.get(index).unwrap() {
        Value::Null => None,
        Value::Text(value) => Some(value.clone()),
        value => panic!("expected nullable Text at column {index}, got {value:?}"),
    }
}
