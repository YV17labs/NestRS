use nest_rs_authz::__private::{Uncontained, contained_text, contained_value};
use sea_orm::sea_query::{Condition, Expr, Order, Values};
use sea_orm::{ActiveModelTrait, DbErr, EntityTrait, Iterable, Value};

/// `condition` as one parenthesized operand, so a balanced text holding an `OR` stays
/// beside the scope rather than reaching past it; `None` for an empty `all`, which is
/// `TRUE`. Refused when its text, its values left as placeholders, would not stay
/// inside that operand.
pub(crate) fn contained(condition: Condition) -> Result<Option<Expr>, Uncontained> {
    let expr = Expr::from(condition);
    if matches!(expr, Expr::Constant(Value::Bool(Some(true)))) {
        return Ok(None);
    }
    contained_text(&expr)?;
    Ok(Some(Expr::tuple([expr])))
}

/// Refuses a value of `active` whose enum type name would leave its quotes: the one
/// text a bound value writes into the statement.
pub(crate) fn contained_values<A: ActiveModelTrait>(active: &A) -> Result<(), DbErr> {
    <A::Entity as EntityTrait>::Column::iter()
        .try_for_each(|column| {
            active
                .get(column)
                .into_value()
                .map_or(Ok(()), |value| contained_value(&value))
        })
        .map_err(refused::<A::Entity>)
}

/// Refuses an order by field whose values `contained_value` refuses: sea-query writes
/// them inline, in the statement's `ORDER BY`.
pub(crate) fn contained_order(order: &Order) -> Result<(), Uncontained> {
    match order {
        Order::Field(Values(values)) => values.iter().try_for_each(contained_value),
        _ => Ok(()),
    }
}

/// The refusal of a condition or value of `E`'s, said at `warn` by entity and reason,
/// never by its text: the opaque failure every caller reads, sent before any
/// statement.
pub(crate) fn refused<E: EntityTrait>(why: Uncontained) -> DbErr {
    tracing::warn!(
        target: crate::target::ORM,
        entity = E::default().table_name(),
        reason = why.as_str(),
        "a condition or value handed to Repo refused — its text would not stay where Repo puts it",
    );
    DbErr::Custom(format!(
        "a condition or value handed to Repo was refused before any statement: its text \
         would not stay where Repo puts it ({}) — build it with sea-query's typed \
         calls, its values bound, every parenthesis, string and identifier it opens closed \
         inside it, and no comment",
        why.as_str()
    ))
}
