use tracing::{error, trace};

use crate::{
    map_data::{
        graph::MapDataGraph,
        osm::{
            OsmNode, OsmRelation, OsmRelationMember, OsmRelationMemberRole, OsmRelationMemberType,
            OsmWay,
        },
        MapDataError,
    },
    osm_json_parser::{OsmElement, OsmElementType, OsmJsonParser, OsmJsonParserError},
};
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::PathBuf,
    time::Instant,
};

pub const ALLOWED_HIGHWAY_VALUES: [&str; 17] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "motorway_link",
    "trunk_link",
    "primary_link",
    "secondary_link",
    "tertiary_link",
    "living_street",
    "track",
    "escape",
    "raceway",
    "road",
];

#[derive(Debug, thiserror::Error)]
pub enum OsmDataReaderError {
    #[error("OSM JSON parser error: {error}")]
    ParserError { error: OsmJsonParserError },

    #[error("Map data error: {error}")]
    MapDataError { error: MapDataError },

    #[error("File error: {error}")]
    FileError { error: io::Error },

    #[error("Failed to open PBF file: {error}")]
    PbfFileOpenError { error: io::Error },

    #[error("Failed to read PBF file: {error}")]
    PbfFileReadError { error: osmpbfreader::Error },

    #[error("PBF file error: {error}")]
    PbfFileError { error: String },
}

#[derive(Debug, PartialEq, Clone)]
pub enum DataSource {
    JsonFile { file: PathBuf },
    PbfFile { file: PathBuf },
}

pub struct OsmDataReader {
    source: DataSource,
    map_data: MapDataGraph,
}

impl OsmDataReader {
    pub fn new(data_source: DataSource) -> Self {
        Self {
            map_data: MapDataGraph::new(),
            source: data_source,
        }
    }

    pub fn read_data(mut self) -> Result<MapDataGraph, OsmDataReaderError> {
        match self.source {
            DataSource::JsonFile { ref file } => {
                self.read_json(file.clone())?;
            }
            DataSource::PbfFile { ref file } => {
                self.read_pbf(file.clone())?;
            }
        };
        Ok(self.map_data)
    }

    fn process_elements(&mut self, elements: Vec<OsmElement>) -> Result<(), OsmDataReaderError> {
        for element in elements {
            match element
                .get_element_type()
                .map_err(|error| OsmDataReaderError::ParserError { error })?
            {
                OsmElementType::Node => {
                    let node = element
                        .get_node_element()
                        .map_err(|error| OsmDataReaderError::ParserError { error })?;
                    self.map_data.insert_node(node);
                }
                OsmElementType::Way => {
                    let way = element
                        .get_way_element()
                        .map_err(|error| OsmDataReaderError::ParserError { error })?;
                    let res = self
                        .map_data
                        .insert_way(way)
                        .map_err(|error| OsmDataReaderError::MapDataError { error });
                    if let Err(error) = res {
                        error!(error=?error, "Error, skipping way");
                    }
                }
                OsmElementType::Relation => {
                    let rel = element
                        .get_relation_element()
                        .map_err(|error| OsmDataReaderError::ParserError { error })?;
                    let res = self
                        .map_data
                        .insert_relation(rel)
                        .map_err(|error| OsmDataReaderError::MapDataError { error });
                    if let Err(error) = res {
                        error!(error=?error, "Error, skipping relation");
                    }
                }
            }
        }
        Ok(())
    }

    fn read_pbf(&mut self, file: PathBuf) -> Result<(), OsmDataReaderError> {
        let read_start = Instant::now();

        let path = std::path::Path::new(&file);
        let r = std::fs::File::open(path)
            .map_err(|error| OsmDataReaderError::PbfFileOpenError { error })?;
        let mut pbf = osmpbfreader::OsmPbfReader::new(r);

        let elements = pbf
            .get_objs_and_deps(|obj| {
                obj.is_way()
                    && obj.tags().iter().any(|t| {
                        t.0 == "highway"
                            && (ALLOWED_HIGHWAY_VALUES.contains(&t.1.as_str())
                                || (t.1 == "path"
                                    && obj
                                        .tags()
                                        .iter()
                                        .any(|t2| t2.0 == "motorcycle" && t2.1 == "yes")))
                    })
                    && !obj.tags().contains("motor_vehicle", "destination")
            })
            .map_err(|error| OsmDataReaderError::PbfFileReadError { error })?;

        for (_id, element) in elements {
            if element.is_node() {
                let node = element.node().ok_or(OsmDataReaderError::PbfFileError {
                    error: String::from("expected node, did not get it"),
                })?;
                self.map_data.insert_node(OsmNode {
                    id: node.id.0 as u64,
                    lat: node.lat(),
                    lon: node.lon(),
                });
            } else if element.is_way() {
                let way = element.way().ok_or(OsmDataReaderError::PbfFileError {
                    error: String::from("expected way, did not get it"),
                })?;
                self.map_data
                    .insert_way(OsmWay {
                        id: way.id.0 as u64,
                        point_ids: way.nodes.iter().map(|v| v.0 as u64).collect(),
                        tags: Some(
                            way.tags
                                .iter()
                                .map(|v| (v.0.to_string(), v.1.to_string()))
                                .collect(),
                        ),
                    })
                    .map_err(|error| OsmDataReaderError::MapDataError { error })?;
            } else if element.is_relation() {
                let relation = element.relation().ok_or(OsmDataReaderError::PbfFileError {
                    error: String::from("expected relation, did not get it"),
                })?;
                self.map_data
                    .insert_relation(OsmRelation {
                        id: relation.id.0 as u64,
                        members: relation
                            .refs
                            .iter()
                            .map(|v| -> Result<OsmRelationMember, OsmDataReaderError> {
                                Ok(OsmRelationMember {
                                    member_ref: match v.member {
                                        osmpbfreader::OsmId::Way(id) => id.0 as u64,
                                        osmpbfreader::OsmId::Node(id) => id.0 as u64,
                                        osmpbfreader::OsmId::Relation(id) => id.0 as u64,
                                    },
                                    role: match v.role.as_str() {
                                        "from" => OsmRelationMemberRole::From,
                                        "to" => OsmRelationMemberRole::To,
                                        "via" => OsmRelationMemberRole::Via,
                                        _ => Err(OsmDataReaderError::PbfFileError {
                                            error: String::from("unknown role"),
                                        })?,
                                    },
                                    member_type: match v.member {
                                        osmpbfreader::OsmId::Way(_) => OsmRelationMemberType::Way,
                                        osmpbfreader::OsmId::Node(_) => OsmRelationMemberType::Node,
                                        _ => Err(OsmDataReaderError::PbfFileError {
                                            error: String::from("unexpected member type"),
                                        })?,
                                    },
                                })
                            })
                            .collect::<Result<Vec<OsmRelationMember>, OsmDataReaderError>>()?,
                        tags: relation
                            .tags
                            .iter()
                            .map(|v| (v.0.to_string(), v.1.to_string()))
                            .collect(),
                    })
                    .map_err(|error| OsmDataReaderError::MapDataError { error })?;
            }
        }

        self.map_data.generate_point_hashes();

        let read_duration = read_start.elapsed();
        trace!("file read took {} seconds", read_duration.as_secs());

        Ok(())
    }

    fn read_json(&mut self, file: PathBuf) -> Result<(), OsmDataReaderError> {
        let read_start = Instant::now();
        let mut parser_state = OsmJsonParser::new();

        let f = File::open(file).map_err(|error| OsmDataReaderError::FileError { error })?;
        let mut reader = BufReader::new(f);
        loop {
            let mut line = String::new();
            let len = reader
                .read_line(&mut line)
                .map_err(|error| OsmDataReaderError::FileError { error })?;
            if len == 0 {
                break;
            }
            let line = line.as_bytes().to_owned();
            let elements = parser_state
                .parse_line(line)
                .map_err(|error| OsmDataReaderError::ParserError { error })?;
            self.process_elements(elements)?;
        }

        self.map_data.generate_point_hashes();

        let read_duration = read_start.elapsed();
        trace!(
            read_duration_secs = read_duration.as_secs(),
            "File read done"
        );

        Ok(())
    }
}
