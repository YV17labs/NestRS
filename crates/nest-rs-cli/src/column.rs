//! The columns a generator takes on its command line — `title:string`,
//! `bio:text:nullable` — and every place one lands: the entity's field, the
//! migration's `ColumnDef` and `DeriveIden` variant, the round trip's body and
//! assertion. [`read_model`] reads them back off an entity's `Model`, so the
//! migration `g migration` writes for an entity is the one `g resource` wrote
//! beside it.

use heck::{ToSnakeCase, ToUpperCamelCase};
use syn::{Fields, GenericArgument, Item, LitStr, PathArguments, Type};

/// What a column holds, by the word the command line spells it with — the
/// words Rails' and Phoenix's generators share, each mapped onto the type
/// SeaORM's prelude names and the `ColumnDef` method creating its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnType {
    /// `String`, a `varchar`.
    String,
    /// `String`, a `text`.
    Text,
    /// `i32`.
    Integer,
    /// `i64`.
    Bigint,
    /// `f64`, a `double precision`.
    Float,
    /// `bool`.
    Boolean,
    /// `Uuid`.
    Uuid,
    /// `Date`, a calendar day.
    Date,
    /// `DateTimeWithTimeZone`, an instant: the house timestamps' type.
    Datetime,
}

impl ColumnType {
    /// Every type, in the order a refusal lists their words.
    const ALL: [Self; 9] = [
        Self::String,
        Self::Text,
        Self::Integer,
        Self::Bigint,
        Self::Float,
        Self::Boolean,
        Self::Uuid,
        Self::Date,
        Self::Datetime,
    ];

    /// The word the command line spells it with.
    fn word(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Bigint => "bigint",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Uuid => "uuid",
            Self::Date => "date",
            Self::Datetime => "datetime",
        }
    }

    /// The field's type, as `sea_orm::entity::prelude` names it.
    fn rust(self) -> &'static str {
        match self {
            Self::String | Self::Text => "String",
            Self::Integer => "i32",
            Self::Bigint => "i64",
            Self::Float => "f64",
            Self::Boolean => "bool",
            Self::Uuid => "Uuid",
            Self::Date => "Date",
            Self::Datetime => "DateTimeWithTimeZone",
        }
    }

    /// The type a field's Rust type reads back as; `text` when its
    /// `column_type` says so, since both are a `String`.
    fn of_rust(rust: &str, text: bool) -> Option<Self> {
        match rust {
            "String" if text => Some(Self::Text),
            _ if text => None,
            _ => Self::ALL
                .into_iter()
                .find(|ty| *ty != Self::Text && ty.rust() == rust),
        }
    }

    /// The `ColumnDef` method that types its column.
    fn def(self) -> &'static str {
        match self {
            Self::String => "string()",
            Self::Text => "text()",
            Self::Integer => "integer()",
            Self::Bigint => "big_integer()",
            Self::Float => "double()",
            Self::Boolean => "boolean()",
            Self::Uuid => "uuid()",
            Self::Date => "date()",
            Self::Datetime => "timestamp_with_time_zone()",
        }
    }

    /// A value of this type, as JSON writes it and the wire type reads it back.
    fn sample(self, name: &str) -> String {
        match self {
            Self::String | Self::Text => {
                let words = name.replace('_', " ");
                let mut chars = words.chars();
                let first = chars.next().map(|c| c.to_ascii_uppercase());
                format!(
                    "\"{}{} 1\"",
                    first.into_iter().collect::<String>(),
                    chars.as_str()
                )
            }
            Self::Integer | Self::Bigint => "1".to_owned(),
            Self::Float => "1.5".to_owned(),
            Self::Boolean => "true".to_owned(),
            Self::Uuid => "\"00000000-0000-0000-0000-000000000001\"".to_owned(),
            Self::Date => "\"2026-01-01\"".to_owned(),
            // chrono writes an instant at offset zero with `Z`, as Postgres
            // hands every `timestamptz` back.
            Self::Datetime => "\"2026-01-01T00:00:00Z\"".to_owned(),
        }
    }
}

/// The columns every generated table holds, which no list may name again:
/// its key, and the three `timestamps` and `soft_delete` read.
const HOUSE: [&str; 4] = ["id", "created_at", "updated_at", "deleted_at"];

/// One column a resource declares beside the house columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Column {
    name: String,
    ty: ColumnType,
    nullable: bool,
}

impl Column {
    /// The column a resource declares when its command names none.
    pub(crate) fn placeholder() -> Self {
        Self {
            name: "name".to_owned(),
            ty: ColumnType::String,
            nullable: false,
        }
    }

    /// Every column `specs` declares, in order — the placeholder alone when
    /// it declares none; the first spec the grammar does not read refuses the
    /// whole list.
    pub(crate) fn parse_all(specs: &[String]) -> Result<Vec<Self>, String> {
        if specs.is_empty() {
            return Ok(vec![Self::placeholder()]);
        }
        let mut columns: Vec<Self> = Vec::with_capacity(specs.len());
        for spec in specs {
            let column = Self::parse(spec)?;
            if columns.iter().any(|seen| seen.name == column.name) {
                return Err(format!("`{}` is named twice", column.name));
            }
            columns.push(column);
        }
        Ok(columns)
    }

    /// `<name>:<type>[:nullable]`.
    fn parse(spec: &str) -> Result<Self, String> {
        let mut parts = spec.split(':');
        let name = parts.next().unwrap_or_default();
        check_name(name)?;
        let Some(word) = parts.next() else {
            return Err(format!(
                "`{name}` names no type: a column is `<name>:<type>[:nullable]` — `{name}:string` — \
                 its type one of {}",
                type_words().join(", ")
            ));
        };
        let ty = ColumnType::ALL
            .into_iter()
            .find(|ty| ty.word() == word)
            .ok_or_else(|| {
                format!(
                    "`{spec}` takes no type `{word}`: a column's type is one of {}",
                    type_words().join(", ")
                )
            })?;
        let nullable = match (parts.next(), parts.next()) {
            (None, _) => false,
            (Some("nullable"), None) => true,
            _ => {
                return Err(format!(
                    "`{spec}` is not a column: after its type, a column takes `nullable` alone"
                ));
            }
        };
        Ok(Self {
            name: name.to_owned(),
            ty,
            nullable,
        })
    }

    /// The `Model` field, with the attributes that expose it as an input of
    /// both writes, a non-null string holding at least one character.
    pub(crate) fn field(&self) -> String {
        let mut field = String::new();
        if self.ty == ColumnType::Text {
            field.push_str("    #[sea_orm(column_type = \"Text\")]\n");
        }
        let string = matches!(self.ty, ColumnType::String | ColumnType::Text);
        if string && !self.nullable {
            field.push_str("    #[expose(input(create, update), validate(length(min = 1)))]\n");
        } else {
            field.push_str("    #[expose(input(create, update))]\n");
        }
        let rust = self.ty.rust();
        let ty = if self.nullable {
            format!("Option<{rust}>")
        } else {
            rust.to_owned()
        };
        field.push_str(&format!("    pub {}: {ty},\n", self.name));
        field
    }

    /// The `"name": value` member a request body carries for it.
    fn member(&self) -> String {
        format!("\"{}\": {}", self.name, self.ty.sample(&self.name))
    }

    /// The round trip's check that the value read back is the one written.
    fn assertion(&self) -> String {
        let (name, sample) = (&self.name, self.ty.sample(&self.name));
        match self.ty {
            ColumnType::Integer | ColumnType::Bigint => {
                format!("    assert_eq!(fields.get(\"{name}\").i64(), {sample});\n")
            }
            ColumnType::Float => {
                format!("    assert_eq!(fields.get(\"{name}\").deserialize::<f64>(), {sample});\n")
            }
            ColumnType::Boolean => {
                format!("    assert!(fields.get(\"{name}\").deserialize::<bool>());\n")
            }
            ColumnType::String
            | ColumnType::Text
            | ColumnType::Uuid
            | ColumnType::Date
            | ColumnType::Datetime => {
                format!("    assert_eq!(fields.get(\"{name}\").string(), {sample});\n")
            }
        }
    }
}

/// Refuse a column name a field, a JSON member and an SQL column cannot all
/// spell alike.
fn check_name(name: &str) -> Result<(), String> {
    let words: Vec<&str> = name.split('_').collect();
    let snake = words.iter().all(|word| {
        word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    });
    if !snake {
        return Err(format!(
            "`{name}` is not a column name: snake_case words, each starting with a lowercase \
             letter (`published_at`)"
        ));
    }
    if syn::parse_str::<syn::Ident>(name).is_err() {
        return Err(format!("`{name}` is a Rust keyword, which no field takes"));
    }
    if HOUSE.contains(&name) {
        return Err(format!(
            "`{name}` is a column every table holds already: {}",
            HOUSE.map(|house| format!("`{house}`")).join(", ")
        ));
    }
    if name == "table" {
        return Err("`table` names the table itself in the migration's identifier enum".to_owned());
    }
    Ok(())
}

/// Every type word, in the order a refusal lists them.
pub(crate) fn type_words() -> [&'static str; 9] {
    ColumnType::ALL.map(ColumnType::word)
}

/// The entity fields `columns` declare, in order.
pub(crate) fn fields(columns: &[Column]) -> String {
    columns.iter().map(Column::field).collect()
}

/// The JSON object a round trip posts: one line when it fits, else a member
/// per line.
pub(crate) fn body(columns: &[Column]) -> String {
    let members: Vec<String> = columns.iter().map(Column::member).collect();
    let line = format!("{{ {} }}", members.join(", "));
    // `        .body_json(&json!(` and `))`: rustfmt leaves a macro's object as
    // written, so the line is kept within its width here.
    if line.len() + 29 <= 100 {
        return line;
    }
    let indented: Vec<String> = members
        .iter()
        .map(|member| format!("            {member}"))
        .collect();
    format!("{{\n{}\n        }}", indented.join(",\n"))
}

/// The round trip's checks, one per column.
pub(crate) fn assertions(columns: &[Column]) -> String {
    columns.iter().map(Column::assertion).collect()
}

/// A column of the table a migration creates, with what its builder adds to
/// its type.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableColumn {
    column: Column,
    /// The primary key.
    key: bool,
    /// A creation or update stamp, defaulting to the insert's instant.
    stamped: bool,
}

impl TableColumn {
    fn plain(column: Column) -> Self {
        let stamped = column.ty == ColumnType::Datetime
            && !column.nullable
            && matches!(column.name.as_str(), "created_at" | "updated_at");
        Self {
            column,
            key: false,
            stamped,
        }
    }

    /// Its `DeriveIden` variant, and the `iden` its name needs when heck,
    /// which `DeriveIden` snake-cases the variant with, would not give it back.
    fn variant(&self) -> (String, Option<&str>) {
        let name = &self.column.name;
        let variant = name.to_upper_camel_case();
        let iden = (variant.to_snake_case() != *name).then_some(name.as_str());
        (variant, iden)
    }

    /// The builder's `.col(…)`, laid out as rustfmt lays a chain out: on one
    /// line within its 60-column chain width, else a call per line.
    fn builder(&self, singular: &str) -> String {
        let (variant, _) = self.variant();
        let head = format!("ColumnDef::new({singular}::{variant})");
        let mut calls = vec![format!(".{}", self.column.ty.def())];
        calls.push(
            if self.column.nullable {
                ".null()"
            } else {
                ".not_null()"
            }
            .to_owned(),
        );
        if self.key {
            calls.push(".primary_key()".to_owned());
        }
        if self.stamped {
            calls.push(".default(Expr::current_timestamp())".to_owned());
        }
        let chain = format!("{head}{}", calls.concat());
        if chain.len() <= 60 {
            return format!("                    .col({chain})\n");
        }
        let calls: String = calls
            .iter()
            .map(|call| format!("\n                            {call}"))
            .collect();
        format!(
            "                    .col(\n                        {head}{calls},\n                    )\n"
        )
    }
}

/// The columns of a table, in the order its migration creates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table(Vec<TableColumn>);

impl Table {
    /// A generated entity's table: its key, `columns`, then the three house
    /// columns `timestamps` and `soft_delete` read.
    pub(crate) fn around(columns: &[Column]) -> Self {
        let house = |name: &str, nullable: bool| {
            TableColumn::plain(Column {
                name: name.to_owned(),
                ty: ColumnType::Datetime,
                nullable,
            })
        };
        let mut table = vec![TableColumn {
            column: Column {
                name: "id".to_owned(),
                ty: ColumnType::Uuid,
                nullable: false,
            },
            key: true,
            stamped: false,
        }];
        table.extend(columns.iter().cloned().map(TableColumn::plain));
        table.extend([
            house("created_at", false),
            house("updated_at", false),
            house("deleted_at", true),
        ]);
        Self(table)
    }

    /// The builder's `.col(…)` lines, its key first, over the enum
    /// `singular`; `todo` marks after the key where the columns are to come.
    pub(crate) fn builder(&self, singular: &str, todo: bool) -> String {
        let mut lines = String::new();
        for (i, column) in self.0.iter().enumerate() {
            lines.push_str(&column.builder(singular));
            if todo && i == 0 {
                lines.push_str(
                    "                    // TODO: add your columns here, and as a variant of the enum below.\n",
                );
            }
        }
        lines
    }

    /// The identifier enum's variants after `Table`, one per column.
    pub(crate) fn variants(&self) -> String {
        self.0
            .iter()
            .map(|column| match column.variant() {
                (variant, None) => format!("    {variant},\n"),
                (variant, Some(iden)) => {
                    format!("    #[sea_orm(iden = \"{iden}\")]\n    {variant},\n")
                }
            })
            .collect()
    }
}

/// A `Model` field the builder cannot type, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Untyped {
    /// The field's name.
    pub field: String,
    /// What the builder cannot read: its type, or an attribute.
    pub what: String,
}

/// What an entity's `Model` declares of its table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelColumns {
    /// The columns the builder types, in the field order.
    pub table: Table,
    /// The fields it leaves to the developer.
    pub untyped: Vec<Untyped>,
}

/// The `table_name = "…"` an entity's source declares.
pub(crate) fn table_name(source: &str) -> Option<String> {
    let (_, rest) = source.split_once("table_name = \"")?;
    let (table, _) = rest.split_once('"')?;
    (!table.is_empty()).then(|| table.to_owned())
}

/// The columns an entity's `pub struct Model` declares, each field read off
/// its Rust type and its `#[sea_orm(…)]`; a relation or an ignored field is no
/// column. `None` when the source declares no `Model`.
pub(crate) fn read_model(source: &str) -> Option<ModelColumns> {
    let file = syn::parse_file(source).ok()?;
    let model = file.items.iter().find_map(|item| match item {
        Item::Struct(s) if s.ident == "Model" => Some(s),
        _ => None,
    })?;
    let Fields::Named(fields) = &model.fields else {
        return None;
    };
    let mut table = Vec::new();
    let mut untyped = Vec::new();
    for field in &fields.named {
        let Some(name) = field.ident.as_ref().map(ToString::to_string) else {
            continue;
        };
        match read_field(field) {
            Read::Column { ty, nullable, key } => {
                let mut column = TableColumn::plain(Column { name, ty, nullable });
                column.key = key;
                table.push(column);
            }
            Read::Untyped(what) => untyped.push(Untyped { field: name, what }),
            Read::NoColumn => {}
        }
    }
    Some(ModelColumns {
        table: Table(table),
        untyped,
    })
}

/// What one `Model` field is to the builder.
enum Read {
    Column {
        ty: ColumnType,
        nullable: bool,
        key: bool,
    },
    Untyped(String),
    NoColumn,
}

/// The `#[sea_orm(…)]` keys a field's column is typed with; any other is the
/// developer's to carry into the builder.
const READ_KEYS: [&str; 3] = ["primary_key", "auto_increment", "column_type"];

/// The keys that make a field no column of its own table.
const NO_COLUMN_KEYS: [&str; 4] = ["ignore", "belongs_to", "has_one", "has_many"];

fn read_field(field: &syn::Field) -> Read {
    let (mut key, mut text) = (false, false);
    let mut unread = None;
    let mut no_column = false;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
        #[expect(
            clippy::disallowed_methods,
            reason = "reads sea-orm's own #[sea_orm] attribute, whose keys are sea-orm's"
        )]
        let parsed = attr.parse_nested_meta(|meta| {
            let ident = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = match meta.value() {
                Ok(value) => Some(value.parse::<syn::Expr>()?),
                Err(_) => None,
            };
            if NO_COLUMN_KEYS.contains(&ident.as_str()) {
                no_column = true;
            } else if ident == "primary_key" {
                key = true;
            } else if ident == "column_type" {
                match value {
                    Some(syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(lit),
                        ..
                    })) if lit_is(&lit, "Text") => text = true,
                    _ => unread = Some("its `column_type`".to_owned()),
                }
            } else if !READ_KEYS.contains(&ident.as_str()) {
                unread = Some(format!("`#[sea_orm({ident})]`"));
            }
            Ok(())
        });
        if parsed.is_err() {
            unread = Some("its `#[sea_orm(…)]`".to_owned());
        }
    }
    let (inner, nullable) = match option_of(&field.ty) {
        Some(inner) => (inner, true),
        None => (&field.ty, false),
    };
    let rust = type_name(inner);
    if no_column || matches!(rust.as_str(), "HasOne<…>" | "HasMany<…>") {
        return Read::NoColumn;
    }
    if let Some(what) = unread {
        return Read::Untyped(what);
    }
    match ColumnType::of_rust(&rust, text) {
        Some(ty) => Read::Column { ty, nullable, key },
        None => Read::Untyped(format!("`{rust}`")),
    }
}

fn lit_is(lit: &LitStr, value: &str) -> bool {
    lit.value() == value
}

/// `T` when `ty` is `Option<T>`.
fn option_of(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

/// A type as a sentence names it: its last segment, `<…>` for its arguments.
fn type_name(ty: &Type) -> String {
    let Type::Path(path) = ty else {
        return "a type".to_owned();
    };
    path.path
        .segments
        .last()
        .map(|last| match last.arguments {
            PathArguments::None => last.ident.to_string(),
            _ => format!("{}<…>", last.ident),
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(specs: &[&str]) -> Vec<String> {
        specs.iter().map(|spec| (*spec).to_owned()).collect()
    }

    #[test]
    fn a_list_reads_each_column_in_order_and_none_is_the_placeholder() {
        let columns =
            Column::parse_all(&specs(&["title:string", "bio:text:nullable"])).expect("two columns");
        assert_eq!(
            columns,
            [
                Column {
                    name: "title".into(),
                    ty: ColumnType::String,
                    nullable: false
                },
                Column {
                    name: "bio".into(),
                    ty: ColumnType::Text,
                    nullable: true
                },
            ]
        );
        assert_eq!(
            Column::parse_all(&[]).expect("none"),
            [Column::placeholder()]
        );
    }

    #[test]
    fn every_type_word_reads_back_as_its_type() {
        for ty in ColumnType::ALL {
            let spec = format!("value:{}", ty.word());
            assert_eq!(Column::parse(&spec).expect("a word").ty, ty);
            assert_eq!(
                ColumnType::of_rust(ty.rust(), ty == ColumnType::Text),
                Some(ty),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_spec_the_grammar_does_not_read_is_refused_with_why() {
        for (spec, says) in [
            ("title", "names no type"),
            ("title:strin", "string, text, integer"),
            ("title:string:null", "`nullable` alone"),
            ("title:string:nullable:x", "`nullable` alone"),
            ("Title:string", "snake_case"),
            ("title__x:string", "snake_case"),
            ("1st:string", "snake_case"),
            ("x_1:string", "snake_case"),
            ("type:string", "keyword"),
            ("id:uuid", "every table"),
            ("deleted_at:datetime", "every table"),
            ("table:string", "identifier enum"),
        ] {
            let refused = Column::parse(spec).expect_err(spec);
            assert!(refused.contains(says), "{spec}: {refused}");
        }
        let twice =
            Column::parse_all(&specs(&["title:string", "title:text"])).expect_err("a name twice");
        assert!(twice.contains("named twice"), "{twice}");
    }

    #[test]
    fn a_field_carries_the_attributes_its_type_takes() {
        let field = |spec: &str| Column::parse(spec).expect(spec).field();
        assert_eq!(
            field("title:string"),
            "    #[expose(input(create, update), validate(length(min = 1)))]\n    pub title: String,\n"
        );
        assert_eq!(
            field("body:text"),
            "    #[sea_orm(column_type = \"Text\")]\n    #[expose(input(create, update), validate(length(min = 1)))]\n    pub body: String,\n"
        );
        assert_eq!(
            field("bio:string:nullable"),
            "    #[expose(input(create, update))]\n    pub bio: Option<String>,\n"
        );
        assert_eq!(
            field("due_on:date"),
            "    #[expose(input(create, update))]\n    pub due_on: Date,\n"
        );
    }

    #[test]
    fn the_builder_lays_a_long_chain_out_as_rustfmt_does() {
        let table = Table::around(&[]);
        let builder = table.builder("Post", false);
        assert!(
            builder.contains(
                "                    .col(ColumnDef::new(Post::Id).uuid().not_null().primary_key())\n"
            ),
            "{builder}"
        );
        assert!(
            builder.contains(
                "                    .col(\n                        ColumnDef::new(Post::DeletedAt)\n                            .timestamp_with_time_zone()\n                            .null(),\n                    )\n"
            ),
            "{builder}"
        );
        assert!(
            builder.contains(".default(Expr::current_timestamp()),"),
            "{builder}"
        );
    }

    #[test]
    fn a_name_heck_does_not_give_back_carries_its_iden() {
        let columns = Column::parse_all(&specs(&["pos_x:integer", "x_y:integer"])).expect("two");
        let variants = Table::around(&columns).variants();
        assert!(variants.contains("    PosX,\n"), "{variants}");
        assert!(
            variants.contains("    #[sea_orm(iden = \"x_y\")]\n    XY,\n"),
            "{variants}"
        );
    }

    #[test]
    fn a_short_body_is_one_line_and_a_long_one_a_member_per_line() {
        let short = Column::parse_all(&specs(&["title:string", "body:text"])).expect("two");
        assert_eq!(
            body(&short),
            "{ \"title\": \"Title 1\", \"body\": \"Body 1\" }"
        );
        let long = Column::parse_all(&specs(&[
            "title:string",
            "body:text",
            "views:integer",
            "published:boolean",
            "published_at:datetime",
        ]))
        .expect("five");
        assert!(
            body(&long).starts_with("{\n            \"title\": \"Title 1\",\n"),
            "{}",
            body(&long)
        );
    }

    /// The migration `g migration` writes off an entity is the one `g
    /// resource` writes beside it: the table read back off the rendered
    /// entity is the table the columns make.
    #[test]
    fn a_rendered_entity_reads_back_as_the_table_its_columns_make() {
        let columns = Column::parse_all(&specs(&[
            "title:string",
            "body:text",
            "views:integer",
            "total:bigint",
            "score:float",
            "published:boolean",
            "author_id:uuid",
            "due_on:date",
            "published_at:datetime",
            "bio:text:nullable",
        ]))
        .expect("every type");
        let entity = crate::scaffold::Renderer::new(&crate::naming::Names::parse("posts"))
            .with("columns", fields(&columns))
            .render(crate::templates::resource::ENTITY);

        let read = read_model(&entity).expect("a Model");

        assert_eq!(read.table, Table::around(&columns));
        assert!(read.untyped.is_empty(), "{:?}", read.untyped);
        assert_eq!(table_name(&entity).as_deref(), Some("post"));
    }

    #[test]
    fn a_relation_is_no_column_and_an_unread_type_is_named() {
        let source = r#"
            pub struct Model {
                #[sea_orm(primary_key, auto_increment = false)]
                pub id: Uuid,
                pub status: PostStatus,
                #[sea_orm(unique)]
                pub email: String,
                #[sea_orm(column_type = "String(StringLen::N(64))")]
                pub code: String,
                #[sea_orm(belongs_to, from = "org_id", to = "id")]
                pub org: HasOne<crate::orgs::Entity>,
                #[sea_orm(has_many)]
                pub posts: HasMany<crate::posts::Entity>,
                #[sea_orm(ignore)]
                pub cached: String,
            }
        "#;
        let read = read_model(source).expect("a Model");
        assert_eq!(read.table.0.len(), 1, "{:?}", read.table);
        assert_eq!(
            read.untyped,
            [
                Untyped {
                    field: "status".into(),
                    what: "`PostStatus`".into()
                },
                Untyped {
                    field: "email".into(),
                    what: "`#[sea_orm(unique)]`".into()
                },
                Untyped {
                    field: "code".into(),
                    what: "its `column_type`".into()
                },
            ]
        );
    }
}
