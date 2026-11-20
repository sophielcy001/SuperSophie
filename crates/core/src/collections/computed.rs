// SPDX-License-Identifier: AGPL-3.0-or-later

use super::formula::{self, Expression, Formula, Function, Operator};
use super::schema::{TITLE_FIELD, definition, validate_schema};
use super::{
    Collection, PropertyDefinition, PropertyKind, check_schema_revision, read_collection,
    read_collection_schema,
};
use crate::assets::hex_sha256;
use crate::integrations::Access;
use crate::model::{name, new_id};
use crate::{Error, Result, Store};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

pub(crate) mod cache;

const RULE_VERSION: u32 = 1;
const MAX_STEPS: usize = 2_048;
const MAX_EVALUATION_DEPTH: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Calculation {
    Formula {
        expression: String,
    },
    Rollup {
        relation_property_id: String,
        function: RollupFunction,
        target_property_id: Option<String>,
    },
}

impl Calculation {
    pub(crate) fn kind(&self) -> PropertyKind {
        match self {
            Self::Formula { .. } => PropertyKind::Formula,
            Self::Rollup { .. } => PropertyKind::Rollup,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RollupFunction {
    Count,
    Sum,
    Min,
    Max,
    Average,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputedDraft {
    pub id: Option<String>,
    pub name: String,
    pub calculation: Calculation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComputedError {
    Cycle,
    LimitExceeded,
    Restricted,
    TypeError,
    DivisionByZero,
    OutOfRange,
    InvalidDate,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ComputedValue {
    Value { value: Value },
    Error { code: ComputedError },
}

pub type ComputedValues = BTreeMap<String, ComputedValue>;

#[derive(Debug, Serialize)]
pub struct ComputedPage {
    pub page_id: String,
    pub page_revision: u32,
    pub representation_version: String,
    pub computed_at: i64,
    pub stale: bool,
    pub properties: ComputedValues,
}

impl Store {
    pub fn save_computed_property(
        &self,
        workspace: &str,
        collection: &str,
        schema_revision: u32,
        draft: ComputedDraft,
    ) -> Result<Collection> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut schema = read_collection(&tx, collection)?;
        check_schema_revision(&schema, schema_revision)?;
        let previous = match draft.id.as_deref() {
            Some(id) => Some(
                schema
                    .properties
                    .iter()
                    .position(|field| field.id == id)
                    .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "找不到要修改的属性。"))?,
            ),
            None => None,
        };
        let kind = draft.calculation.kind();
        if previous.is_some_and(|index| schema.properties[index].kind != kind) {
            return Err(Error::new("PROPERTY_TYPE", "已创建属性的类型暂不能更改。"));
        }
        let mut property = PropertyDefinition {
            id: draft.id.unwrap_or_else(new_id),
            name: name(&draft.name, 64, None)?,
            kind,
            options: vec![],
            relation: None,
            calculation: Some(draft.calculation),
        };
        if let Some(Calculation::Formula { expression }) = &mut property.calculation {
            let (canonical, _) = formula::compile(expression.trim(), |input| {
                resolve_field(&schema.properties, &property.id, &property.name, input)
            })?;
            *expression = canonical;
        }
        if previous.is_some_and(|index| schema.properties[index] == property) {
            return Ok(schema);
        }
        let field = property.id.clone();
        if let Some(index) = previous {
            schema.properties[index] = property;
        } else {
            schema.properties.push(property);
        }
        validate_schema(&schema.properties)?;
        validate_rollups(&schema, |id| {
            if id == schema.id {
                Ok(Some(schema.clone()))
            } else if super::relations::deleted_collection(&tx, id)? {
                Ok(None)
            } else {
                read_collection_schema(&tx, id).map(Some)
            }
        })?;
        super::save_schema(&tx, &mut schema, &[&field])?;
        tx.commit()?;
        Ok(schema)
    }
}

fn resolve_field(
    properties: &[PropertyDefinition],
    id: &str,
    label: &str,
    input: &str,
) -> Option<String> {
    if input == TITLE_FIELD || input == "标题" {
        return Some(TITLE_FIELD.into());
    }
    if input == id {
        return Some(id.into());
    }
    if let Some(field) = definition(properties, input) {
        return Some(field.id.clone());
    }
    let input = input.to_lowercase();
    if input == label.to_lowercase() {
        return Some(id.into());
    }
    properties
        .iter()
        .find(|field| field.id != id && field.name.to_lowercase() == input)
        .map(|field| field.id.clone())
}

pub(super) fn compile_formula(
    properties: &[PropertyDefinition],
    expression: &str,
) -> Result<Formula> {
    let (canonical, formula) = formula::compile(expression, |input| {
        (input == TITLE_FIELD || definition(properties, input).is_some()).then(|| input.to_owned())
    })?;
    if canonical != expression {
        return Err(Error::new(
            "FORMULA_SCHEMA",
            "公式的属性引用需要使用稳定标识。",
        ));
    }
    Ok(formula)
}

pub(super) fn validate_calculations(properties: &[PropertyDefinition]) -> Result<()> {
    for field in properties {
        if field.calculation.as_ref().map(Calculation::kind)
            != field.kind.computed().then_some(field.kind)
        {
            return Err(Error::new("COMPUTED_SCHEMA", "计算属性需要匹配的规则。"));
        }
        if let Some(Calculation::Formula { expression }) = &field.calculation {
            compile_formula(properties, expression)?;
        }
    }
    Ok(())
}

pub(super) fn validate_rollups(
    source: &Collection,
    mut read_target: impl FnMut(&str) -> Result<Option<Collection>>,
) -> Result<()> {
    for field in &source.properties {
        let Some(Calculation::Rollup {
            relation_property_id,
            function,
            target_property_id,
        }) = &field.calculation
        else {
            continue;
        };
        let invalid = || Error::new("ROLLUP_SCHEMA", "请选择有效的关联及目标数值属性。");
        let relation = definition(&source.properties, relation_property_id)
            .and_then(|field| field.relation.as_ref())
            .ok_or_else(invalid)?;
        let target = read_target(&relation.collection_id)?;
        match (function, target_property_id) {
            (RollupFunction::Count, None) => {}
            (RollupFunction::Count, Some(_)) | (_, None) => return Err(invalid()),
            (_, Some(id)) => {
                let valid = match &target {
                    Some(target) => definition(&target.properties, id).is_some_and(|field| {
                        field.kind == PropertyKind::Number || field.kind.computed()
                    }),
                    None => crate::model::valid_id(id),
                };
                if !valid {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

pub(super) fn index(tx: &rusqlite::Transaction<'_>, collection: &Collection) -> Result<()> {
    tx.execute(
        "DELETE FROM computed_dependencies WHERE collection_id=?1",
        [&collection.id],
    )?;
    let mut insert = tx.prepare("INSERT INTO computed_dependencies VALUES (?1,?2,?3,?4)")?;
    for (relation, input, output) in dependency_fields(&collection.properties)? {
        insert.execute((&collection.id, input, output, relation))?;
    }
    Ok(())
}

pub(crate) fn dependency_fields(
    properties: &[PropertyDefinition],
) -> Result<Vec<(String, String, String)>> {
    let mut dependencies = Vec::new();
    for field in properties {
        match &field.calculation {
            Some(Calculation::Formula { expression }) => {
                for input in compile_formula(properties, expression)?.fields {
                    dependencies.push((String::new(), input, field.id.clone()));
                }
            }
            Some(Calculation::Rollup {
                relation_property_id,
                target_property_id,
                ..
            }) => {
                dependencies.push((
                    String::new(),
                    relation_property_id.clone(),
                    field.id.clone(),
                ));
                dependencies.push((
                    relation_property_id.clone(),
                    target_property_id.clone().unwrap_or_default(),
                    field.id.clone(),
                ));
            }
            None => {}
        }
    }
    Ok(dependencies)
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RecordInput {
    id: String,
    pub(crate) title: String,
    created_at: i64,
    updated_at: i64,
    pub(crate) active: bool,
    pub(crate) collection: String,
    pub(crate) properties: super::schema::Values,
}

impl RecordInput {
    pub(crate) fn read(db: &Connection, id: &str) -> Result<Option<Self>> {
        Ok(db
            .prepare_cached(
                "SELECT p.id, p.title, r.created_at, p.updated_at, r.collection_id, r.properties,
                    p.trash_root_id IS NULL
             FROM collection_records r JOIN pages p ON p.id=r.page_id WHERE r.page_id=?1",
            )?
            .query_row([id], |row| {
                Ok(Self {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    collection: row.get(4)?,
                    properties: super::json_column(row, 5)?,
                    active: row.get(6)?,
                })
            })
            .optional()?)
    }
}

struct Schema {
    properties: Vec<PropertyDefinition>,
    expressions: HashMap<String, Formula>,
}

pub(crate) struct Engine<'a> {
    db: &'a Connection,
    access: Option<&'a Access>,
    boundary: Option<i64>,
    records: HashMap<String, Option<Rc<RecordInput>>>,
    schemas: HashMap<String, Rc<Schema>>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum Dependency {
    Field(String, String),
    Relation(String, String),
}

#[derive(Serialize)]
enum Input {
    Value(Value),
    Rule(Calculation),
    Error(ComputedError),
}

impl<'a> Engine<'a> {
    pub(crate) fn new(db: &'a Connection, access: Option<&'a Access>) -> Self {
        Self {
            db,
            access,
            boundary: None,
            records: HashMap::new(),
            schemas: HashMap::new(),
        }
    }

    pub(crate) fn historical(db: &'a Connection, access: &'a Access, boundary: i64) -> Self {
        let mut engine = Self::new(db, Some(access));
        engine.boundary = Some(boundary);
        engine
    }

    fn record(&mut self, id: &str) -> Result<Option<Rc<RecordInput>>> {
        if let Some(page) = self.records.get(id) {
            return Ok(page.clone());
        }
        let readable = self.access.map_or(Ok(true), |access| {
            if self.boundary.is_some() {
                // The snapshot supplies the active flag. Job observers must
                // still match the current grant and resource-range revisions.
                access.covers_archived_record(self.db, id)
            } else {
                access.can_read(self.db, id)
            }
        })?;
        let record = if readable {
            let before: Option<Option<String>> = if let Some(boundary) = self.boundary {
                self.db
                    .prepare_cached(
                        "SELECT state FROM computed_record_before
                     WHERE page_id=?1 AND job_id>=?2 ORDER BY job_id LIMIT 1",
                    )?
                    .query_row((id, boundary), |row| row.get(0))
                    .optional()?
            } else {
                None
            };
            let state = match before {
                Some(state) => state.map(|data| serde_json::from_str(&data)).transpose()?,
                None => RecordInput::read(self.db, id)?,
            };
            state.filter(|record| record.active).map(Rc::new)
        } else {
            None
        };
        self.records.insert(id.to_owned(), record.clone());
        Ok(record)
    }

    fn schema(&mut self, id: &str) -> Result<Rc<Schema>> {
        if let Some(schema) = self.schemas.get(id) {
            return Ok(Rc::clone(schema));
        }
        let historical: Option<String> = if let Some(boundary) = self.boundary {
            self.db
                .prepare_cached(
                    "SELECT properties FROM computed_schema_before
                 WHERE collection_id=?1 AND job_id>=?2 ORDER BY job_id LIMIT 1",
                )?
                .query_row((id, boundary), |row| row.get(0))
                .optional()?
        } else {
            None
        };
        let properties: Vec<PropertyDefinition> = match historical {
            Some(json) => serde_json::from_str(&json)?,
            None => read_collection_schema(self.db, id)?.properties,
        };
        let mut expressions = HashMap::new();
        for field in &properties {
            if let Some(Calculation::Formula { expression }) = &field.calculation {
                expressions.insert(field.id.clone(), compile_formula(&properties, expression)?);
            }
        }
        let schema = Rc::new(Schema {
            properties,
            expressions,
        });
        self.schemas.insert(id.to_owned(), Rc::clone(&schema));
        Ok(schema)
    }

    pub(crate) fn fields(&mut self, id: &str) -> Result<(ComputedValues, String)> {
        self.evaluate(id, None)
    }

    fn evaluate(
        &mut self,
        id: &str,
        mut dependencies: Option<&mut BTreeSet<Dependency>>,
    ) -> Result<(ComputedValues, String)> {
        let mut values = ComputedValues::new();
        let mut projection = Sha256::new();
        projection.update(RULE_VERSION.to_be_bytes());
        if let Some(record) = self.record(id)? {
            let schema = self.schema(&record.collection)?;
            for field in schema
                .properties
                .iter()
                .filter(|field| field.calculation.is_some())
            {
                let mut evaluation = Evaluation {
                    engine: self,
                    steps: MAX_STEPS,
                    path: Vec::new(),
                    inputs: Sha256::new(),
                    dependencies: dependencies.as_deref_mut(),
                };
                let value = match evaluation.field(&record, &field.id, 0) {
                    Ok(value) => ComputedValue::Value { value },
                    Err(Fault::Cell(code)) => ComputedValue::Error { code },
                    Err(Fault::Store(error)) => return Err(error),
                };
                projection.update(evaluation.inputs.finalize());
                digest(&mut projection, &(&field.id, &value))?;
                values.insert(field.id.clone(), value);
            }
        }
        Ok((values, hex_sha256(&projection.finalize())))
    }

    fn field_input(&mut self, record: &RecordInput, id: &str) -> Result<Input> {
        if id == TITLE_FIELD {
            return Ok(Input::Value(Value::from(record.title.clone())));
        }
        let schema = self.schema(&record.collection)?;
        let Some(field) = definition(&schema.properties, id) else {
            return Ok(Input::Error(ComputedError::TypeError));
        };
        if field.kind == PropertyKind::Attachment
            && self.access.is_some_and(|access| !access.reads_assets())
        {
            return Ok(Input::Error(ComputedError::Restricted));
        }
        if let Some(calculation) = &field.calculation {
            return Ok(Input::Rule(calculation.clone()));
        }
        let raw = record.properties.get(id).unwrap_or(&Value::Null);
        let value = match field.kind {
            PropertyKind::CreatedAt => Value::from(record.created_at),
            PropertyKind::UpdatedAt => Value::from(record.updated_at),
            PropertyKind::Select | PropertyKind::Status => field
                .options
                .iter()
                .find(|option| raw.as_str() == Some(&option.id))
                .map_or(Value::Null, |option| Value::from(option.name.clone())),
            PropertyKind::Attachment | PropertyKind::Relation | PropertyKind::MultiSelect => {
                return Ok(Input::Error(ComputedError::TypeError));
            }
            _ => raw.clone(),
        };
        Ok(Input::Value(value))
    }

    fn relation_members(
        &mut self,
        record: &RecordInput,
        relation: &str,
    ) -> Result<Vec<Rc<RecordInput>>> {
        let Some(ids) = record.properties.get(relation).and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut ordered = ids.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut members = Vec::new();
        for id in ordered {
            if let Some(member) = self.record(id)? {
                members.push(member);
            }
        }
        Ok(members)
    }
}

enum Fault {
    Cell(ComputedError),
    Store(Error),
}

impl From<ComputedError> for Fault {
    fn from(code: ComputedError) -> Self {
        Self::Cell(code)
    }
}

impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self::Store(error)
    }
}

impl From<rusqlite::Error> for Fault {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

type Evaluated = std::result::Result<Value, Fault>;

struct Evaluation<'a, 'db> {
    engine: &'a mut Engine<'db>,
    steps: usize,
    path: Vec<(String, String)>,
    inputs: Sha256,
    dependencies: Option<&'a mut BTreeSet<Dependency>>,
}

impl Evaluation<'_, '_> {
    fn visit(&mut self, depth: usize) -> std::result::Result<(), ComputedError> {
        if self.steps == 0 || depth >= MAX_EVALUATION_DEPTH {
            return Err(ComputedError::LimitExceeded);
        }
        self.steps -= 1;
        Ok(())
    }

    fn field(&mut self, record: &RecordInput, id: &str, depth: usize) -> Evaluated {
        if self
            .path
            .iter()
            .any(|(page, field)| page == &record.id && field == id)
        {
            return Err(ComputedError::Cycle.into());
        }
        self.visit(depth)?;
        if let Some(dependencies) = self.dependencies.as_deref_mut() {
            dependencies.insert(Dependency::Field(record.id.clone(), id.to_owned()));
        }
        let calculation = match self.engine.field_input(record, id)? {
            Input::Value(value) => {
                digest(&mut self.inputs, &(&record.id, id, &value))?;
                return Ok(value);
            }
            Input::Rule(calculation) => calculation,
            Input::Error(code) => return Err(code.into()),
        };
        digest(&mut self.inputs, &(&record.id, id, &calculation))?;
        self.path.push((record.id.clone(), id.to_owned()));
        let result = match &calculation {
            Calculation::Formula { .. } => {
                let schema = self.engine.schema(&record.collection)?;
                self.expression(record, &schema.expressions[id].expression, depth + 1)
            }
            Calculation::Rollup {
                relation_property_id,
                function,
                target_property_id,
            } => self.rollup(
                record,
                relation_property_id,
                *function,
                target_property_id.as_deref(),
                depth + 1,
            ),
        };
        self.path.pop();
        result
    }

    fn rollup(
        &mut self,
        record: &RecordInput,
        relation: &str,
        function: RollupFunction,
        target_field: Option<&str>,
        depth: usize,
    ) -> Evaluated {
        if let Some(dependencies) = self.dependencies.as_deref_mut() {
            dependencies.insert(Dependency::Relation(record.id.clone(), relation.to_owned()));
        }
        let mut count = 0_u32;
        let mut aggregate = 0.0_f64;
        for target in self.engine.relation_members(record, relation)? {
            self.visit(depth)?;
            let Some(field) = target_field else {
                digest(&mut self.inputs, &target.id)?;
                count += 1;
                continue;
            };
            let value = self.field(&target, field, depth + 1)?;
            if value.is_null() {
                continue;
            }
            let value = value.as_f64().ok_or(ComputedError::TypeError)?;
            aggregate = match (function, count) {
                (RollupFunction::Min | RollupFunction::Max, 0) => value,
                (RollupFunction::Min, _) => aggregate.min(value),
                (RollupFunction::Max, _) => aggregate.max(value),
                _ => aggregate + value,
            };
            count += 1;
        }
        match (function, count) {
            (RollupFunction::Count, _) => Ok(Value::from(count)),
            (RollupFunction::Sum, _) => Ok(formula::number(aggregate)?),
            (_, 0) => Ok(Value::Null),
            (RollupFunction::Average, _) => Ok(formula::number(aggregate / f64::from(count))?),
            _ => Ok(formula::number(aggregate)?),
        }
    }

    fn expression(&mut self, record: &RecordInput, node: &Expression, depth: usize) -> Evaluated {
        self.visit(depth)?;
        let next = depth + 1;
        match node {
            Expression::Literal(value) => Ok(value.clone()),
            Expression::Field(id) => self.field(record, id, next),
            Expression::Unary(op, value) => {
                Ok(formula::unary(*op, self.expression(record, value, next)?)?)
            }
            Expression::Binary(op, left, right) => {
                let left = self.expression(record, left, next)?;
                if matches!(op, Operator::And | Operator::Or) {
                    let left = formula::boolean(&left)?;
                    if (*op == Operator::And && !left) || (*op == Operator::Or && left) {
                        return Ok(Value::Bool(left));
                    }
                    return Ok(Value::Bool(formula::boolean(
                        &self.expression(record, right, next)?,
                    )?));
                }
                Ok(formula::binary(
                    *op,
                    left,
                    self.expression(record, right, next)?,
                )?)
            }
            Expression::Call(Function::If, args) => {
                let condition = formula::boolean(&self.expression(record, &args[0], next)?)?;
                self.expression(record, &args[if condition { 1 } else { 2 }], next)
            }
            Expression::Call(Function::Coalesce, args) => {
                let first = self.expression(record, &args[0], next)?;
                if first.is_null() {
                    self.expression(record, &args[1], next)
                } else {
                    Ok(first)
                }
            }
            Expression::Call(function, args) => {
                let values = args
                    .iter()
                    .map(|node| self.expression(record, node, next))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(formula::apply(*function, &values)?)
            }
        }
    }
}

fn digest(hash: &mut Sha256, input: &impl Serialize) -> Result<()> {
    let json = serde_json::to_vec(input)?;
    hash.update((json.len() as u64).to_be_bytes());
    hash.update(json);
    Ok(())
}
