use sqlx::{
    Error, Execute, Postgres, SqlStr,
    error::BoxDynError,
    postgres::{PgArguments, PgStatement},
};

pub struct Inspected {
    sql: SqlStr,
    statement: Option<PgStatement>,
    arguments: Option<PgArguments>,
    persistent: bool,
}

impl Execute<'_, Postgres> for Inspected {
    fn sql(self) -> SqlStr {
        self.sql
    }

    fn statement(&self) -> Option<&PgStatement> {
        self.statement.as_ref()
    }

    fn take_arguments(&mut self) -> Result<Option<PgArguments>, BoxDynError> {
        Ok(self.arguments.take())
    }

    fn persistent(&self) -> bool {
        self.persistent
    }
}

pub fn inspect<'q>(
    mut query: impl Execute<'q, Postgres>,
    refuse: impl FnOnce(&str) -> Option<BoxDynError>,
) -> Result<Inspected, Error> {
    let arguments = query.take_arguments().map_err(Error::Encode)?;
    let statement = query.statement().cloned();
    let persistent = query.persistent();
    let sql = query.sql();
    if let Some(refused) = refuse(sql.as_str()) {
        return Err(Error::Configuration(refused));
    }
    Ok(Inspected {
        sql,
        statement,
        arguments,
        persistent,
    })
}
