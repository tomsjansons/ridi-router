use crate::{
    map_data::graph::MapDataGraph,
    osm_data::{data_reader::ALLOWED_HIGHWAY_VALUES, pbf_area_reader::PbfAreaReader},
};
use geo::{CoordsIter, Distance, GeodesicArea, Haversine, HaversineClosestPoint, Point};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tracing::info;

use crate::map_data::osm::{
    OsmNode, OsmRelation, OsmRelationMember, OsmRelationMemberRole, OsmRelationMemberType, OsmWay,
};
use std::{path::PathBuf, time::Instant};

use super::OsmDataReaderError;

const RESIDENTIAL_PROXIMITY_THRESHOLD_METERS: f64 = 500.0;
const RESIDENTIAL_PART_COVERED: f64 = 0.10;
const THRESHOLD_AREA: f64 = (RESIDENTIAL_PROXIMITY_THRESHOLD_METERS
    * RESIDENTIAL_PROXIMITY_THRESHOLD_METERS
    * std::f64::consts::PI)
    * RESIDENTIAL_PART_COVERED;
const MILITARY_ENTRY_MAX_M: f64 = 100.;

pub struct PbfReader<'a> {
    map_data: &'a mut MapDataGraph,
    file_name: &'a PathBuf,
}

enum OsmElement {
    Way(OsmWay),
    Node(OsmNode),
    Relation(OsmRelation),
}

impl<'a> PbfReader<'a> {
    pub fn new(map_data: &'a mut MapDataGraph, file_name: &'a PathBuf) -> Self {
        Self {
            map_data,
            file_name,
        }
    }

    pub fn read(self) -> Result<(), OsmDataReaderError> {
        let read_start = Instant::now();

        let r = std::fs::File::open(self.file_name)
            .map_err(|error| OsmDataReaderError::PbfFileOpenError { error })?;
        let mut pbf = osmpbfreader::OsmPbfReader::new(r);

        let mut boundary_reader = PbfAreaReader::new(&mut pbf);
        boundary_reader.read(&|obj| {
            (obj.is_way() || obj.is_relation()) && obj.tags().contains("landuse", "residential")
        })?;
        let residential_area_grid = boundary_reader.get_area_grid();

        let mut boundary_reader = PbfAreaReader::new(&mut pbf);
        boundary_reader.read(&|obj| {
            (obj.is_way() || obj.is_relation()) && obj.tags().contains("landuse", "military")
        })?;
        let military_area_grid = boundary_reader.get_area_grid();

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

        elements
            .par_iter()
            .map(
                |(_element_id, element)| -> Result<OsmElement, OsmDataReaderError> {
                    if element.is_node() {
                        let node = element.node().ok_or(OsmDataReaderError::PbfFileError {
                            error: String::from("expected node, did not get it"),
                        })?;
                        return Ok(OsmElement::Node(OsmNode {
                            id: node.id.0 as u64,
                            lat: node.lat(),
                            lon: node.lon(),
                            residential_in_proximity: {
                                let tot_area = match residential_area_grid.find_closest_areas_refs(
                                    node.lat() as f32,
                                    node.lon() as f32,
                                    1,
                                ) {
                                    Some(areas) => areas.iter().fold(0., |tot, multi_polygon| {
                                        let geo_point = Point::new(node.lon(), node.lat());
                                        let distance = match multi_polygon
                                            .haversine_closest_point(&geo_point)
                                        {
                                            geo::Closest::Intersection(_) => 0.,
                                            geo::Closest::SinglePoint(p) => {
                                                Haversine.distance(p, geo_point)
                                            }
                                            geo::Closest::Indeterminate => multi_polygon
                                                .coords_iter()
                                                .fold(10000., |min, coords| {
                                                    let dist = Haversine
                                                        .distance(geo_point, Point::from(coords));
                                                    if dist < min {
                                                        dist
                                                    } else {
                                                        min
                                                    }
                                                }),
                                        };
                                        if distance <= RESIDENTIAL_PROXIMITY_THRESHOLD_METERS {
                                            let area = multi_polygon.geodesic_area_signed().abs();
                                            return tot + area;
                                        }
                                        tot
                                    }),
                                    None => 0.,
                                };

                                tot_area > THRESHOLD_AREA
                            },
                            nogo_area: match military_area_grid.find_closest_areas_refs(
                                node.lat() as f32,
                                node.lon() as f32,
                                1,
                            ) {
                                None => false,
                                Some(areas) => areas.iter().any(|multi_polygon| {
                                    let geo_point = Point::new(node.lon(), node.lat());
                                    match multi_polygon.haversine_closest_point(&geo_point) {
                                        geo::Closest::Intersection(p) => {
                                            // only mark as nogo if inside a military area more than 100m
                                            // this is to account for data oddities where a road may
                                            // techcnally be in a military zone but on the outer edge and
                                            // ok to be on. but this will prevent from choosing roads that
                                            // go deeper into the area
                                            Haversine.distance(geo_point, p) > MILITARY_ENTRY_MAX_M
                                        }
                                        geo::Closest::SinglePoint(_) => false,
                                        geo::Closest::Indeterminate => false,
                                    }
                                }),
                            },
                        }));
                    } else if element.is_way() {
                        let way = element.way().ok_or(OsmDataReaderError::PbfFileError {
                            error: String::from("expected way, did not get it"),
                        })?;
                        return Ok(OsmElement::Way(OsmWay {
                            id: way.id.0 as u64,
                            point_ids: way.nodes.iter().map(|v| v.0 as u64).collect(),
                            tags: Some(
                                way.tags
                                    .iter()
                                    .map(|v| (v.0.to_string(), v.1.to_string()))
                                    .collect(),
                            ),
                        }));
                    } else if element.is_relation() {
                        let relation =
                            element.relation().ok_or(OsmDataReaderError::PbfFileError {
                                error: String::from("expected relation, did not get it"),
                            })?;
                        return Ok(OsmElement::Relation(OsmRelation {
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
                                            osmpbfreader::OsmId::Way(_) => {
                                                OsmRelationMemberType::Way
                                            }
                                            osmpbfreader::OsmId::Node(_) => {
                                                OsmRelationMemberType::Node
                                            }
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
                        }));
                    }
                    Err(OsmDataReaderError::UnexpectedElement)
                },
            )
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|osm_element| -> Result<(), OsmDataReaderError> {
                match osm_element {
                    OsmElement::Node(node) => self.map_data.insert_node(node),
                    OsmElement::Way(way) => self
                        .map_data
                        .insert_way(way)
                        .map_err(|error| OsmDataReaderError::MapDataError { error })?,
                    OsmElement::Relation(relation) => self
                        .map_data
                        .insert_relation(relation)
                        .map_err(|error| OsmDataReaderError::MapDataError { error })?,
                };
                Ok(())
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.map_data.generate_point_hashes();

        let read_duration = read_start.elapsed();
        info!(read_duration = read_duration.as_secs(), "File read done");

        Ok(())
    }
}
