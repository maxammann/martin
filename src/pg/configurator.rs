use crate::pg::config::{
    FuncInfoSources, FunctionInfo, PgConfig, PgInfo, TableInfo, TableInfoSources,
};
use crate::pg::function_source::get_function_sources;
use crate::pg::pg_source::{PgSource, PgSqlInfo};
use crate::pg::pool::Pool;
use crate::pg::table_source::{calc_srid, get_table_sources, merge_table_info, table_to_query};
use crate::source::IdResolver;
use crate::srv::server::Sources;
use crate::utils::{find_info, InfoMap};
use futures::future::{join_all, try_join};
use itertools::Itertools;
use log::{debug, error, info, warn};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;

pub async fn resolve_pg_data(
    config: PgConfig,
    id_resolver: IdResolver,
) -> io::Result<(Sources, PgConfig, Pool)> {
    let pg = PgBuilder::new(&config, id_resolver).await?;
    let ((mut tables, tbl_info), (funcs, func_info)) =
        try_join(pg.instantiate_tables(), pg.instantiate_functions()).await?;

    tables.extend(funcs);
    Ok((
        tables,
        PgConfig {
            tables: tbl_info,
            functions: func_info,
            ..config
        },
        pg.pool,
    ))
}

struct PgBuilder {
    pool: Pool,
    default_srid: Option<i32>,
    discover_functions: bool,
    discover_tables: bool,
    id_resolver: IdResolver,
    tables: TableInfoSources,
    functions: FuncInfoSources,
}

impl PgBuilder {
    async fn new(config: &PgConfig, id_resolver: IdResolver) -> io::Result<Self> {
        let pool = Pool::new(config).await?;
        Ok(Self {
            pool,
            default_srid: config.default_srid,
            discover_functions: config.discover_functions,
            discover_tables: config.discover_tables,
            id_resolver,
            tables: config.tables.clone(),
            functions: config.functions.clone(),
        })
    }

    pub async fn instantiate_tables(&self) -> Result<(Sources, TableInfoSources), io::Error> {
        let all_tables = get_table_sources(&self.pool).await?;

        dbg!(&all_tables);

        // Match configured sources with the discovered ones and add them to the pending list.
        let mut used = HashSet::<(&str, &str, &str)>::new();
        let mut pending = Vec::new();
        for (id, cfg_inf) in &self.tables {
            // TODO: move this validation to serde somehow?
            if let Some(extent) = cfg_inf.extent {
                if extent == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("Configuration for source {id} has extent=0"),
                    ));
                }
            }

            let Some(schemas) = find_info(&all_tables, &cfg_inf.schema, "schema", id) else { continue };
            let Some(tables) = find_info(schemas, &cfg_inf.table, "table", id) else { continue };
            let Some(src_inf) = find_info(tables, &cfg_inf.geometry_column, "geometry column", id) else { continue };

            let dup = used.insert((&cfg_inf.schema, &cfg_inf.table, &cfg_inf.geometry_column));
            let dup = if dup { "duplicate " } else { "" };

            let id2 = self.resolve_id(id.clone(), cfg_inf);
            let Some(cfg_inf) = merge_table_info(self.default_srid,&id2, cfg_inf, src_inf) else { continue };
            warn_on_rename(id, &id2, "table");
            info!("Configured {dup}source {id2} from {}", summary(&cfg_inf));
            pending.push(table_to_query(id2, cfg_inf, self.pool.clone()));
        }

        if self.discover_tables {
            // Sort the discovered sources by schema, table and geometry column to ensure a consistent behavior
            for (schema, tables) in all_tables.into_iter().sorted_by(by_key) {
                for (table, geoms) in tables.into_iter().sorted_by(by_key) {
                    for (geom, mut src_inf) in geoms.into_iter().sorted_by(by_key) {
                        if used.contains(&(schema.as_str(), table.as_str(), geom.as_str())) {
                            continue;
                        }
                        let id2 = self.resolve_id(table.clone(), &src_inf);
                        let Some(srid) = calc_srid(&src_inf.format_id(), &id2,  src_inf.srid,0, self.default_srid) else {continue};
                        src_inf.srid = srid;
                        info!("Discovered source {id2} from {}", summary(&src_inf));
                        pending.push(table_to_query(id2, src_inf, self.pool.clone()));
                    }
                }
            }
        }

        let mut res: Sources = HashMap::new();
        let mut info_map = TableInfoSources::new();
        let pending = join_all(pending).await;
        for src in pending {
            match src {
                Err(v) => {
                    error!("Failed to create a source: {v}");
                    continue;
                }
                Ok((id, pg_sql, src_inf)) => {
                    debug!("{id} query: {}", pg_sql.query);
                    self.add_func_src(&mut res, id.clone(), &src_inf, pg_sql.clone());
                    info_map.insert(id, src_inf);
                }
            }
        }

        Ok((res, info_map))
    }

    pub async fn instantiate_functions(&self) -> Result<(Sources, FuncInfoSources), io::Error> {
        let mut discovered_sources = get_function_sources(&self.pool).await?;
        let mut res: Sources = HashMap::new();
        let mut info_map = FuncInfoSources::new();
        let mut used: HashMap<String, HashMap<String, PgSqlInfo>> = HashMap::new();

        for (id, cfg_inf) in &self.functions {
            let schema = &cfg_inf.schema;
            let name = &cfg_inf.function;
            if let Some((pg_sql, _)) = discovered_sources
                .get_mut(schema)
                .and_then(|v| v.remove(name))
            {
                // Store it just in case another source needs the same function
                used.entry(schema.to_string())
                    .or_default()
                    .insert(name.to_string(), pg_sql.clone());

                let id2 = self.resolve_id(id.clone(), cfg_inf);
                self.add_func_src(&mut res, id2.clone(), cfg_inf, pg_sql.clone());
                warn_on_rename(id, &id2, "function");
                info!("Configured source {id2} from function {}", pg_sql.signature);
                debug!("{}", pg_sql.query);
                info_map.insert(id2, cfg_inf.clone());
            } else if let Some(pg_sql) = used.get_mut(schema).and_then(|v| v.get(name)) {
                // This function was already used by another source
                let id2 = self.resolve_id(id.clone(), cfg_inf);
                self.add_func_src(&mut res, id2.clone(), cfg_inf, pg_sql.clone());
                warn_on_rename(id, &id2, "function");
                let sig = &pg_sql.signature;
                info!("Configured duplicate source {id2} from function {sig}");
                debug!("{}", pg_sql.query);
                info_map.insert(id2, cfg_inf.clone());
            } else {
                warn!(
                    "Configured function source {id} from {schema}.{name} does not exist or \
                    does not have an expected signature like (z,x,y) -> bytea. See README.md",
                );
            }
        }

        if self.discover_functions {
            // Sort the discovered sources by schema and function name to ensure a consistent behavior
            for (_, funcs) in discovered_sources.into_iter().sorted_by(by_key) {
                for (name, (pg_sql, src_inf)) in funcs.into_iter().sorted_by(by_key) {
                    let id2 = self.resolve_id(name.clone(), &src_inf);
                    self.add_func_src(&mut res, id2.clone(), &src_inf, pg_sql.clone());
                    info!("Discovered source {id2} from function {}", pg_sql.signature);
                    debug!("{}", pg_sql.query);
                    info_map.insert(id2, src_inf);
                }
            }
        }

        Ok((res, info_map))
    }

    fn resolve_id<T: PgInfo>(&self, id: String, src_inf: &T) -> String {
        let signature = format!("{}.{}", self.pool.get_id(), src_inf.format_id());
        self.id_resolver.resolve(id, signature)
    }

    fn add_func_src(&self, sources: &mut Sources, id: String, info: &impl PgInfo, sql: PgSqlInfo) {
        let source = PgSource::new(id.clone(), sql, info.to_tilejson(), self.pool.clone());
        sources.insert(id, Box::new(source));
    }
}

fn warn_on_rename(old_id: &String, new_id: &String, typ: &str) {
    if old_id != new_id {
        warn!("Configured {typ} source {old_id} was renamed to {new_id} due to ID conflict");
    }
}

fn summary(info: &TableInfo) -> String {
    format!(
        "table {}.{} with {} column ({}, SRID={})",
        info.schema,
        info.table,
        info.geometry_column,
        info.geometry_type
            .as_deref()
            .unwrap_or("UNKNOWN GEOMETRY TYPE"),
        info.srid,
    )
}

fn by_key<T>(a: &(String, T), b: &(String, T)) -> Ordering {
    a.0.cmp(&b.0)
}

pub type SqlFuncInfoMapMap = InfoMap<InfoMap<(PgSqlInfo, FunctionInfo)>>;
pub type SqlTableInfoMapMapMap = InfoMap<InfoMap<InfoMap<TableInfo>>>;
