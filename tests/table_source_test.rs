use std::collections::HashMap;

use ctor::ctor;
use indoc::indoc;
use martin::Xyz;

#[path = "utils.rs"]
mod utils;
#[allow(clippy::wildcard_imports)]
use utils::*;

#[ctor]
fn init() {
    let _ = env_logger::builder().is_test(true).try_init();
}

#[actix_rt::test]
async fn table_source() {
    let mock = mock_sources(mock_cfg("connection_string: $DATABASE_URL")).await;
    assert!(!mock.0.is_empty());

    let source = table(&mock, "table_source");
    assert_eq!(source.schema, "public");
    assert_eq!(source.table, "table_source");
    assert_eq!(source.srid, 4326);
    assert_eq!(source.geometry_column, "geom");
    assert_eq!(source.id_column, None);
    assert_eq!(source.minzoom, None);
    assert_eq!(source.maxzoom, None);
    assert!(source.bounds.is_some());
    assert_eq!(source.extent, Some(4096));
    assert_eq!(source.buffer, Some(64));
    assert_eq!(source.clip_geom, Some(true));
    assert_eq!(source.geometry_type, some("GEOMETRY"));

    let mut properties = HashMap::new();
    properties.insert("gid".to_owned(), "int4".to_owned());
    assert_eq!(source.properties, properties);
}

#[actix_rt::test]
async fn tables_tilejson_ok() {
    let mock = mock_sources(mock_cfg("connection_string: $DATABASE_URL")).await;
    let tilejson = source(&mock, "table_source").get_tilejson();

    assert_eq!(tilejson.tilejson, "2.2.0");
    assert_eq!(tilejson.version, some("1.0.0"));
    assert_eq!(tilejson.name, some("public.table_source.geom"));
    assert_eq!(tilejson.scheme, some("xyz"));
    assert_eq!(tilejson.minzoom, Some(0));
    assert_eq!(tilejson.maxzoom, Some(30));
    assert!(tilejson.bounds.is_some());
    assert!(tilejson.tiles.is_empty());
}

#[actix_rt::test]
async fn tables_tile_ok() {
    let mock = mock_sources(mock_cfg("connection_string: $DATABASE_URL")).await;
    let tile = source(&mock, "table_source")
        .get_tile(&Xyz { z: 0, x: 0, y: 0 }, &None)
        .await
        .unwrap();

    assert!(!tile.is_empty());
}

#[actix_rt::test]
async fn tables_srid_ok() {
    let mock = mock_sources(mock_cfg(indoc! {"
        connection_string: $DATABASE_URL
        default_srid: 900913
    "}))
    .await;

    let source = table(&mock, "points1");
    assert_eq!(source.srid, 4326);

    let source = table(&mock, "points2");
    assert_eq!(source.srid, 4326);

    let source = table(&mock, "points3857");
    assert_eq!(source.srid, 3857);

    let source = table(&mock, "points_empty_srid");
    assert_eq!(source.srid, 900_913);
}

#[actix_rt::test]
async fn tables_multiple_geom_ok() {
    let mock = mock_sources(mock_cfg("connection_string: $DATABASE_URL")).await;

    let source = table(&mock, "table_source_multiple_geom");
    assert_eq!(source.geometry_column, "geom1");

    let source = table(&mock, "table_source_multiple_geom.1");
    assert_eq!(source.geometry_column, "geom2");
}

#[actix_rt::test]
async fn table_source_schemas() {
    let cfg = mock_cfg(indoc! {"
        connection_string: $DATABASE_URL
        auto_publish:
          tables:
            from_schema: MixedCase
          functions: false
    "});
    let sources = mock_sources(cfg).await.0;
    assert_eq!(sources.keys().collect::<Vec<_>>(), vec!["MixPoints"],);
}
