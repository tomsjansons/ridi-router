use clap::{arg, value_parser, Command};
use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};

use core::panic;
use std::{
    cmp::Eq,
    collections::{BTreeMap, HashMap},
    fmt::{Debug, Display},
    hash::Hash,
    marker::PhantomData,
};

use geo::HaversineDistance;
use geo::Point;

use crate::{
    cli::Cli,
    gps_hash::{get_gps_coords_hash, HashOffset},
    map_data::{
        osm::{OsmRelationMember, OsmRelationMemberRole, OsmRelationMemberType},
        rule::MapDataRule,
    },
    osm_data_reader::OsmDataReader,
    MAP_DATA_GRAPH,
};

use super::{
    line::MapDataLine,
    osm::{OsmNode, OsmRelation, OsmWay},
    point::MapDataPoint,
    rule::MapDataRuleType,
    way::{MapDataWay, MapDataWayPoints},
    MapDataError,
};

trait MapDataElement: Debug {
    fn get(idx: usize) -> &'static Self;
}
impl MapDataElement for MapDataPoint {
    fn get(idx: usize) -> &'static MapDataPoint {
        &MapDataGraph::get().points[idx]
    }
}
impl MapDataElement for MapDataLine {
    fn get(idx: usize) -> &'static MapDataLine {
        &MapDataGraph::get().lines[idx]
    }
}
impl MapDataElement for MapDataWay {
    fn get(idx: usize) -> &'static MapDataWay {
        &MapDataGraph::get().ways[idx]
    }
}

pub struct MapDataElementRef<T: MapDataElement> {
    idx: usize,
    _marker: PhantomData<T>,
}

impl<T: MapDataElement> MapDataElementRef<T> {
    fn new(idx: usize) -> Self {
        Self {
            idx,
            _marker: PhantomData,
        }
    }

    pub fn borrow(&self) -> &'static T {
        T::get(self.idx)
    }
}

impl<T: MapDataElement> Clone for MapDataElementRef<T> {
    fn clone(&self) -> Self {
        Self {
            idx: self.idx,
            _marker: self._marker,
        }
    }
}

impl<T: MapDataElement> PartialEq for MapDataElementRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl<T: MapDataElement> Eq for MapDataElementRef<T> {}

impl<T: MapDataElement> Hash for MapDataElementRef<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.idx.hash(state)
    }
}

impl<T: MapDataElement + 'static> Debug for MapDataElementRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.borrow().fmt(f)
    }
}

pub type MapDataLineRef = MapDataElementRef<MapDataLine>;
pub type MapDataPointRef = MapDataElementRef<MapDataPoint>;
pub type MapDataWayRef = MapDataElementRef<MapDataWay>;

type PointMap = BTreeMap<u64, MapDataPointRef>;

pub struct MapDataGraph {
    points: Vec<MapDataPoint>,
    points_map: HashMap<u64, usize>,
    point_hashed_offset_none: PointMap,
    point_hashed_offset_lat: PointMap,
    nodes_hashed_offset_lon: PointMap,
    nodes_hashed_offset_lat_lon: PointMap,
    ways: Vec<MapDataWay>,
    ways_map: HashMap<u64, usize>,
    lines: Vec<MapDataLine>,
}

fn get_line_id(way_id: &u64, point_id_1: &u64, point_id_2: &u64) -> String {
    format!("{}-{}-{}", way_id, point_id_1, point_id_2)
}

impl MapDataGraph {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            points_map: HashMap::new(),
            point_hashed_offset_none: BTreeMap::new(),
            point_hashed_offset_lat: BTreeMap::new(),
            nodes_hashed_offset_lon: BTreeMap::new(),
            nodes_hashed_offset_lat_lon: BTreeMap::new(),
            ways: Vec::new(),
            ways_map: HashMap::new(),
            lines: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn test_get_point_ref_by_id(&self, id: &u64) -> Option<MapDataPointRef> {
        self.get_point_ref_by_id(id)
    }

    fn get_point_ref_by_id(&self, id: &u64) -> Option<MapDataPointRef> {
        match self.points_map.get(id) {
            None => return None,
            Some(i) => Some(MapDataElementRef::new(i.clone())),
        }
    }

    fn get_way_ref_by_id(&self, id: &u64) -> Option<MapDataWayRef> {
        match self.ways_map.get(id) {
            None => return None,
            Some(i) => Some(MapDataElementRef::new(i.clone())),
        }
    }

    pub fn insert_node(&mut self, value: OsmNode) -> () {
        let lat = value.lat.clone();
        let lon = value.lon.clone();
        let point = MapDataPoint {
            id: value.id,
            lat: value.lat,
            lon: value.lon,
            part_of_ways: Vec::new(),
            lines: Vec::new(),
            junction: false,
            rules: Vec::new(),
        };
        let idx = self.add_point(point.clone());
        let point_ref = MapDataElementRef::new(idx);
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::None),
            point_ref.clone(),
        );
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::Lat),
            point_ref.clone(),
        );
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::Lon),
            point_ref.clone(),
        );
        self.point_hashed_offset_none
            .insert(get_gps_coords_hash(lat, lon, HashOffset::LatLon), point_ref);
    }

    pub fn generate_point_hashes(&mut self) -> () {
        for point in self.points.iter().filter(|p| !p.lines.is_empty()) {
            let point_idx = self
                .points_map
                .get(&point.id)
                .expect("Point must exist in the points map, something went very wrong");
            let point_ref = MapDataElementRef::new(*point_idx);
            self.point_hashed_offset_none.insert(
                get_gps_coords_hash(point.lat, point.lon, HashOffset::None),
                point_ref.clone(),
            );
            self.point_hashed_offset_none.insert(
                get_gps_coords_hash(point.lat, point.lon, HashOffset::Lat),
                point_ref.clone(),
            );
            self.point_hashed_offset_none.insert(
                get_gps_coords_hash(point.lat, point.lon, HashOffset::Lon),
                point_ref.clone(),
            );
            self.point_hashed_offset_none.insert(
                get_gps_coords_hash(point.lat, point.lon, HashOffset::LatLon),
                point_ref,
            );
        }
    }

    fn get_way_by_idx(&self, idx: usize) -> &MapDataWay {
        &self.ways[idx]
    }
    fn get_point_by_idx(&self, idx: usize) -> &MapDataPoint {
        &self.points[idx]
    }
    fn get_line_by_idx(&self, idx: usize) -> &MapDataLine {
        &self.lines[idx]
    }
    fn get_mut_way_by_idx(&mut self, idx: usize) -> &mut MapDataWay {
        &mut self.ways[idx]
    }
    fn get_mut_point_by_idx(&mut self, idx: usize) -> &mut MapDataPoint {
        &mut self.points[idx]
    }
    fn get_mut_line_by_idx(&mut self, idx: usize) -> &mut MapDataLine {
        &mut self.lines[idx]
    }
    fn add_line(&mut self, line: MapDataLine) -> usize {
        self.lines.push(line);
        self.lines.len() - 1
    }
    fn add_point(&mut self, point: MapDataPoint) -> usize {
        let idx = self.points.len();
        self.points_map.insert(point.id, idx);
        self.points.push(point);
        idx
    }
    fn add_way(&mut self, way: MapDataWay) -> usize {
        let idx = self.ways.len();
        self.ways_map.insert(way.id, idx);
        self.ways.push(way);
        idx
    }

    fn way_is_ok(&self, osm_way: &OsmWay) -> bool {
        if let Some(tags) = &osm_way.tags {
            if tags.get("service").is_some() {
                return false;
            }
            if let Some(access) = tags.get("access") {
                if access == "no" || access == "private" {
                    return false;
                }
            }
            if let Some(motor_vehicle) = tags.get("motor_vehicle") {
                if motor_vehicle == "private" || motor_vehicle == "no" {
                    return false;
                }
            }
            if let Some(highway) = tags.get("highway") {
                return highway != "proposed"
                    && highway != "cycleway"
                    && highway != "steps"
                    && highway != "pedestrian"
                    && highway != "path"
                    && highway != "service"
                    && highway != "footway";
            }
        }
        false
    }

    pub fn insert_way(&mut self, osm_way: OsmWay) -> Result<(), MapDataError> {
        if !self.way_is_ok(&osm_way) {
            return Ok(());
        }
        let mut prev_point_ref: Option<MapDataPointRef> = None;
        let way = MapDataWay {
            id: osm_way.id,
            points: MapDataWayPoints::new(),
        };

        let way_idx = self.add_way(way.clone());
        let way_ref = MapDataElementRef::new(way_idx);
        for point_id in &osm_way.point_ids {
            if let Some(point_ref) = self.get_point_ref_by_id(&point_id) {
                let way_mut = self.get_mut_way_by_idx(way_idx);
                way_mut.points.add(point_ref.clone());
            }

            if let Some(point_ref) = self.get_point_ref_by_id(&point_id) {
                let point_mut = self.get_mut_point_by_idx(point_ref.idx);
                point_mut.part_of_ways.push(way_ref.clone());
            }

            if let Some(point_ref) = self.get_point_ref_by_id(&point_id) {
                if let Some(prev_point_ref) = prev_point_ref {
                    let line_id = get_line_id(
                        &way.id,
                        &self.get_point_by_idx(prev_point_ref.idx).id,
                        &point_id,
                    );
                    let line = MapDataLine {
                        id: line_id,
                        way: way_ref.clone(),
                        points: (prev_point_ref.clone(), point_ref.clone()),
                        one_way: osm_way.is_one_way(),
                        roundabout: osm_way.is_roundabout(),
                        tags_name: osm_way
                            .tags
                            .as_ref()
                            .map_or(None, |t| t.get("name").cloned()),
                        tags_ref: osm_way
                            .tags
                            .as_ref()
                            .map_or(None, |t| t.get("ref").cloned()),
                    };
                    let line_idx = self.add_line(line);
                    let line_ref = MapDataLineRef::new(line_idx);

                    let point_mut = self.get_mut_point_by_idx(point_ref.idx);
                    point_mut.lines.push(line_ref.clone());
                    point_mut.junction = point_mut.lines.len() > 2;

                    let prev_point_mut = self.get_mut_point_by_idx(prev_point_ref.idx);
                    prev_point_mut.lines.push(line_ref);

                    prev_point_mut.junction = prev_point_mut.lines.len() > 2;
                }
                prev_point_ref = Some(point_ref);
            } else {
                return Err(MapDataError::MissingPoint {
                    point_id: point_id.clone(),
                });
            }
        }

        Ok(())
    }

    fn relation_is_ok(&self, relation: &OsmRelation) -> bool {
        if let Some(rel_type) = relation.tags.get("type") {
            // https://wiki.openstreetmap.org/w/index.php?title=Relation:restriction&uselang=en
            // currently only "restriction", but "restriction:bus" was in use until 2013
            if rel_type.starts_with("restriction") {
                let restriction = relation
                    .tags
                    .get("restriction")
                    .or(relation.tags.get("restriction:motorcycle"))
                    .or(relation.tags.get("restriction:conditional"))
                    .or(relation.tags.get("restriction:motorcar"));
                if restriction.is_some() {
                    return true;
                }
            }
        }
        false
    }

    pub fn insert_relation(&mut self, relation: OsmRelation) -> Result<(), MapDataError> {
        if !self.relation_is_ok(&relation) {
            return Ok(());
        }
        let restriction = relation
            .tags
            .get("restriction")
            .or(relation.tags.get("restriction:motorcycle"))
            .or(relation.tags.get("restriction:conditional"))
            .or(relation.tags.get("restriction:motorcar"))
            .ok_or(MapDataError::MissingRestriction {
                osm_relation: relation.clone(),
                relation_id: relation.id,
            })?;
        let rule_type = match restriction.split(" ").collect::<Vec<_>>().get(0) {
            Some(&"no_right_turn") => MapDataRuleType::NotAllowed,
            Some(&"no_left_turn") => MapDataRuleType::NotAllowed,
            Some(&"no_u_turn") => MapDataRuleType::NotAllowed,
            Some(&"no_straight_on") => MapDataRuleType::NotAllowed,
            Some(&"no_entry") => MapDataRuleType::NotAllowed,
            Some(&"no_exit") => MapDataRuleType::NotAllowed,
            Some(&"only_right_turn") => MapDataRuleType::OnlyAllowed,
            Some(&"only_left_turn") => MapDataRuleType::OnlyAllowed,
            Some(&"only_u_turn") => MapDataRuleType::OnlyAllowed,
            Some(&"only_straight_on") => MapDataRuleType::OnlyAllowed,
            _ => {
                return Err(MapDataError::UnknownRestriction {
                    relation_id: relation.id,
                    restriction: restriction.to_string(),
                })
            }
        };

        let via_members = relation
            .members
            .iter()
            .filter(|member| member.role == OsmRelationMemberRole::Via)
            .collect::<Vec<_>>();
        if via_members.len() == 1 {
            fn get_way_refs(
                graph: &MapDataGraph,
                members: &Vec<OsmRelationMember>,
                role: OsmRelationMemberRole,
            ) -> Vec<MapDataWayRef> {
                members
                    .iter()
                    .filter_map(|member| {
                        if member.role == role {
                            return Some(member.member_ref);
                        }
                        None
                    })
                    .filter_map(|w_id| graph.get_way_ref_by_id(&w_id))
                    .collect::<Vec<_>>()
            }
            fn get_lines_from_way_ids(
                graph: &MapDataGraph,
                way_refs: &Vec<MapDataWayRef>,
                point: &MapDataPointRef,
            ) -> Vec<MapDataLineRef> {
                way_refs
                    .iter()
                    .filter_map(|way_ref| {
                        // we may not find a way and that's fine as the
                        // relation may be associated with a service road
                        // or other way that is outside of our dataset
                        graph
                            .get_point_by_idx(point.idx)
                            .lines
                            .iter()
                            .find(|line| {
                                // can't use borrow() here because we are still just setting up the
                                // graph and it'll block itself
                                graph.get_line_by_idx(line.idx).way.idx == way_ref.idx
                            })
                            .cloned()
                    })
                    .collect()
            }
            let from_way_refs = get_way_refs(self, &relation.members, OsmRelationMemberRole::From);
            let to_way_refs = get_way_refs(self, &relation.members, OsmRelationMemberRole::To);

            if from_way_refs.is_empty() || to_way_refs.is_empty() {
                return Ok(());
            }

            let via_member = via_members.first().ok_or(MapDataError::MissingViaMember {
                relation_id: relation.id,
            })?;
            if via_member.member_type == OsmRelationMemberType::Way {
                return Err(MapDataError::NotYetImplemented {
                    message: String::from("restrictions with Ways as the Via role"),
                    relation: relation.clone(),
                });
            }
            let via_point = self.get_point_ref_by_id(&via_member.member_ref).ok_or(
                MapDataError::MissingViaPoint {
                    relation_id: relation.id,
                    point_id: via_member.member_ref,
                },
            )?;

            let from_lines = get_lines_from_way_ids(self, &from_way_refs, &via_point);
            let to_lines = get_lines_from_way_ids(self, &to_way_refs, &via_point);
            let point = self.get_mut_point_by_idx(via_point.idx);
            let rule = MapDataRule {
                from_lines,
                to_lines,
                rule_type,
            };
            point.rules.push(rule);
        } else if via_members.len() > 1 {
            return Err(MapDataError::NotYetImplemented {
                message: String::from("not yet implemented relations with via ways"),
                relation: relation.clone(),
            });
        }
        // relations with a missing via member are invalid and therefore we skip them
        // https://wiki.openstreetmap.org/wiki/Relation:restriction#Members
        Ok(())
    }

    pub fn get_adjacent(
        &self,
        center_point: MapDataPointRef,
    ) -> Vec<(MapDataLineRef, MapDataPointRef)> {
        center_point
            .borrow()
            .lines
            .iter()
            .map(|line| {
                let other_point = if line.borrow().points.0 == center_point {
                    line.borrow().points.1.clone()
                } else {
                    line.borrow().points.0.clone()
                };
                (line.clone(), other_point)
            })
            .collect()
    }

    pub fn get_closest_to_coords(&self, lat: f64, lon: f64) -> Option<MapDataPointRef> {
        let search_hash = get_gps_coords_hash(lat, lon, HashOffset::None);
        let mut grid_points = HashMap::new();

        for level in 0..=32 {
            let shift_width = 2 * level;
            let from = search_hash >> shift_width << shift_width;
            let to = from
                | if shift_width > 0 {
                    u64::max_value() >> (64 - shift_width)
                } else {
                    search_hash
                };

            let offset_none_points = self.point_hashed_offset_none.range(from..=to);
            let offset_lat_points = self.point_hashed_offset_lat.range(from..=to);
            let offset_lon_points = self.nodes_hashed_offset_lon.range(from..=to);
            let offset_lat_lon_points = self.nodes_hashed_offset_lat_lon.range(from..=to);
            let points: [Vec<MapDataPointRef>; 4] = [
                offset_none_points.map(|(_, point)| point.clone()).collect(),
                offset_lat_points.map(|(_, point)| point.clone()).collect(),
                offset_lon_points.map(|(_, point)| point.clone()).collect(),
                offset_lat_lon_points
                    .map(|(_, point)| point.clone())
                    .collect(),
            ];

            let points = points.concat();
            if !points.is_empty() || (from == 0 && to == u64::max_value()) {
                points.iter().for_each(|p| {
                    let id: u64 = p.borrow().id.clone();
                    grid_points.insert(id, p.clone());
                });
                break;
            }
        }

        if grid_points.len() == 1 {
            let point = grid_points.values().next().map(|p| p.clone());
            return point;
        }

        let mut points_with_dist: Vec<(u32, MapDataPointRef)> = grid_points
            .iter()
            .map(|(_, p)| {
                let point1 = Point::new(p.borrow().lon, p.borrow().lat);
                let point2 = Point::new(lon, lat);
                let distance = point1.haversine_distance(&point2);
                (distance.round() as u32, p.clone())
            })
            .collect();

        points_with_dist.sort_by(|(dist_a, _), (dist_b, _)| dist_a.cmp(dist_b));
        points_with_dist.get(0).map(|(_, p)| p.clone())
    }

    pub fn get() -> &'static MapDataGraph {
        MAP_DATA_GRAPH.get_or_init(|| {
            let data_source = match Cli::get() {
                Cli::Single {
                    data_source,
                    from_to: _,
                } => data_source,
                cli => panic!("{:#?} not yet implemented", cli),
            };

            let data_reader = OsmDataReader::new(data_source);
            let map_data = data_reader.read_data().unwrap();

            map_data
        })
    }
}

#[cfg(test)]
mod tests {
    use core::panic;
    use std::{collections::HashSet, u8};

    use rusty_fork::rusty_fork_test;

    use crate::test_utils::{graph_from_test_dataset, set_graph_static, test_dataset_1};

    use super::*;

    #[test]
    fn check_way_ok() {
        let map_data = MapDataGraph::new();
        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "primary".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), true);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "proposed".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "cycleway".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "hhhighway".to_string(),
                "primary".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "steps".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "pedestrian".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([("highway".to_string(), "path".to_string())])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "service".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "footway".to_string(),
            )])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([("highway".to_string(), "omg".to_string())])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), true);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), true);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "no".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "private".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("service".to_string(), "yes".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "yes".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), true);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "no".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "private".to_string()),
            ])),
        };

        assert_eq!(map_data.way_is_ok(&osm_way), false);
    }

    #[derive(Debug)]
    struct PointTest {
        lat: f64,
        lon: f64,
        ways: Vec<u64>,
        lines: Vec<&'static str>,
        junction: bool,
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn check_point_consistency() {
            fn point_is_ok(map_data: &MapDataGraph, id: &u64, test: PointTest) -> bool {
                let point = map_data
                    .get_point_ref_by_id(id)
                    .expect(format!("point {} must exist", id).as_str());
                let point = point.borrow();
                eprintln!("point {:#?}", point);
                eprintln!("test {:#?}", test);
                point.lat == test.lat
                    && point.lon == test.lon
                    && point.part_of_ways.len() == test.ways.len()
                    && point.part_of_ways.iter().enumerate().all(|(idx, w)| {
                        let test_way_id = test
                            .ways
                            .get(idx)
                            .expect(format!("{}: way at idx {} must exist", id, idx).as_str());
                        w.borrow().id == *test_way_id
                    })
                    && point.lines.len() == test.lines.len()
                    && point.lines.iter().enumerate().all(|(idx, l)| {
                        let test_line_id = test
                            .lines
                            .get(idx)
                            .expect(format!("{}: line at idx {} must exist", id, idx).as_str());
                        l.borrow().id == *test_line_id
                    })
                    && point.junction == test.junction
            }
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));
            assert!(point_is_ok(
                &map_data,
                &1,
                PointTest {
                    lat: 1.0,
                    lon: 1.0,
                    ways: vec![1234],
                    lines: vec!["1234-1-2"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &2,
                PointTest {
                    lat: 2.0,
                    lon: 2.0,
                    ways: vec![1234],
                    lines: vec!["1234-1-2", "1234-2-3"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &3,
                PointTest {
                    lat: 3.0,
                    lon: 3.0,
                    ways: vec![1234, 5367],
                    lines: vec!["1234-2-3", "1234-3-4", "5367-5-3", "5367-3-6"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &4,
                PointTest {
                    lat: 4.0,
                    lon: 4.0,
                    ways: vec![1234, 489],
                    lines: vec!["1234-3-4", "489-4-8"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &5,
                PointTest {
                    lat: 5.0,
                    lon: 5.0,
                    ways: vec![5367],
                    lines: vec!["5367-5-3"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &6,
                PointTest {
                    lat: 6.0,
                    lon: 6.0,
                    ways: vec![5367, 68],
                    lines: vec!["5367-3-6", "5367-6-7", "68-6-8"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &7,
                PointTest {
                    lat: 7.0,
                    lon: 7.0,
                    ways: vec![5367],
                    lines: vec!["5367-6-7"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &8,
                PointTest {
                    lat: 8.0,
                    lon: 8.0,
                    ways: vec![489, 68],
                    lines: vec!["489-4-8", "489-8-9", "68-6-8"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &9,
                PointTest {
                    lat: 9.0,
                    lon: 9.0,
                    ways: vec![489],
                    lines: vec!["489-8-9"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &11,
                PointTest {
                    lat: 11.0,
                    lon: 11.0,
                    ways: vec![1112],
                    lines: vec!["1112-11-12"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                &map_data,
                &12,
                PointTest {
                    lat: 12.0,
                    lon: 12.0,
                    ways: vec![1112],
                    lines: vec!["1112-11-12"],
                    junction: false
                }
            ));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn check_way_consistency() {
            fn way_is_ok(map_data: &MapDataGraph, id: &u64, test_points: Vec<u64>) -> bool {
                let way = map_data
                    .ways
                    .iter()
                    .find(|w| w.id == *id)
                    .expect(format!("way {} must exist", id).as_str());
                eprintln!("way {:#?}", way);
                eprintln!("test {:#?}", test_points);
                way.points.len() == test_points.len()
                    && way.points.iter().enumerate().all(|(idx, p)| {
                        let p = p.borrow();
                        p.id == *test_points
                            .get(idx)
                            .expect(format!("point at idx {} must exist", idx).as_str())
                    })
            }
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));

            assert!(way_is_ok(&map_data, &1234, vec![1, 2, 3, 4]));
            assert!(way_is_ok(&map_data, &5367, vec![5, 3, 6, 7]));
            assert!(way_is_ok(&map_data, &489, vec![4, 8, 9]));
            assert!(way_is_ok(&map_data, &68, vec![6, 8]));
            assert!(way_is_ok(&map_data, &1112, vec![11, 12]));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn check_line_consistency() {
            fn line_is_ok(
                map_data: &MapDataGraph,
                id: &str,
                test_way: u64,
                test_points: (u64, u64),
            ) -> bool {
                let line = map_data
                    .lines
                    .iter()
                    .find(|l| l.id == *id)
                    .expect(format!("line {} must exist", id).as_str());
                eprintln!("line {:#?}", line);
                eprintln!("test {:#?}", test_points);
                line.way.borrow().id == test_way
                    && line.points.0.borrow().id == test_points.0
                    && line.points.1.borrow().id == test_points.1
            }
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));
            assert!(line_is_ok(&map_data, "1234-1-2", 1234, (1, 2)));
            assert!(line_is_ok(&map_data, "1234-2-3", 1234, (2, 3)));
            assert!(line_is_ok(&map_data, "1234-3-4", 1234, (3, 4)));
            assert!(line_is_ok(&map_data, "5367-5-3", 5367, (5, 3)));
            assert!(line_is_ok(&map_data, "5367-3-6", 5367, (3, 6)));
            assert!(line_is_ok(&map_data, "5367-6-7", 5367, (6, 7)));
            assert!(line_is_ok(&map_data, "489-4-8", 489, (4, 8)));
            assert!(line_is_ok(&map_data, "489-8-9", 489, (8, 9)));
            assert!(line_is_ok(&map_data, "68-6-8", 68, (6, 8)));
            assert!(line_is_ok(&map_data, "1112-11-12", 1112, (11, 12)));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn check_missing_points() {
            let mut map_data = MapDataGraph::new();
            let res = map_data.insert_way(OsmWay {
                id: 1,
                point_ids: vec![1],
                tags:Some(HashMap::from([("highway".to_string(), "primary".to_string())]))
            });
            if let Ok(_) = res {
                assert!(false);
            } else if let Err(e) = res {
                if let MapDataError::MissingPoint { point_id: p } = e {
                    assert_eq!(p, 1);
                } else {
                    assert!(false);
                }
            }
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn mark_junction() {
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let point = map_data.get_point_ref_by_id(&5).unwrap();
            let points = map_data.get_adjacent(point);
            points.iter().for_each(|p| {
                assert!((p.1.borrow().id == 3 && p.1.borrow().junction == true) || p.1.borrow().id != 3)
            });

            let point = map_data.get_point_ref_by_id(&3).unwrap();
            let points = map_data.get_adjacent(point);
            let non_junctions = vec![2, 5, 4];
            points.iter().for_each(|p| {
                assert!(
                    ((non_junctions.contains(&p.1.borrow().id) && p.1.borrow().junction == false)
                        || !non_junctions.contains(&p.1.borrow().id))
                )
            });
            points.iter().for_each(|p| {
                assert!((p.1.borrow().id == 6 && p.1.borrow().junction == true) || p.1.borrow().id != 6)
            });
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn adjacent_lookup() {
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));

            let tests: Vec<(u8, MapDataPointRef, Vec<(String, u64)>)> = vec![
                (
                    1,
                    MapDataGraph::get().get_point_ref_by_id(&2).unwrap(),
                    vec![(String::from("1234-1-2"), 1), (String::from("1234-2-3"), 3)],
                ),
                (
                    2,
                    MapDataGraph::get().get_point_ref_by_id(&3).unwrap(),
                    vec![
                        (String::from("5367-5-3"), 5),
                        (String::from("5367-6-3"), 6),
                        (String::from("1234-2-3"), 2),
                        (String::from("1234-4-3"), 4),
                    ],
                ),
                (
                    3,
                    MapDataGraph::get().get_point_ref_by_id(&1).unwrap(),
                    vec![(String::from("1234-1-2"), 2)],
                ),
            ];

            for test in tests {
                let (_test_id, point, expected_result) = test;
                let adj_elements = map_data.get_adjacent(point);
                assert_eq!(adj_elements.len(), expected_result.len());
                for (adj_line, adj_point) in &adj_elements {
                    let adj_match = expected_result.iter().find(|&(line_id, point_id)| {
                        line_id.split("-").collect::<HashSet<_>>()
                            == adj_line.borrow().id.split("-").collect::<HashSet<_>>()
                            && point_id == &adj_point.borrow().id
                    });
                    assert_eq!(adj_match.is_some(), true);
                }
            }
        }
    }

    type ClosestTest = ([Option<OsmNode>; 4], OsmNode, u64);

    const CLOSEST_TESTS: [ClosestTest; 8] = [
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.1640,
                    lon: 24.8652,
                }),
                None,
                None,
                None,
            ],
            OsmNode {
                id: 0,
                lat: 57.1670,
                lon: 24.8658,
            },
            1,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.1640,
                    lon: 24.8652,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.1740,
                    lon: 24.8630,
                }),
                None,
                None,
            ],
            OsmNode {
                id: 0,
                lat: 57.1670,
                lon: 24.8658,
            },
            1,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                None,
                None,
            ],
            OsmNode {
                id: 0,
                lat: 57.163429387682214,
                lon: 24.87742424011231,
            },
            2,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                None,
                None,
            ],
            OsmNode {
                id: 0,
                lat: 57.193343289610794,
                lon: 24.872531890869144,
            },
            1,
        ),
        (
            [
                // 57.16961885299059,24.875192642211914
                // 10231.8212 km
                // 223.61
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                // 57.159484808175435,24.877617359161377
                // 10231.6372 km
                // 223.61
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                None,
                None,
            ],
            // -10.660607953624762,-52.03125
            OsmNode {
                id: 0,
                lat: -10.660607953624762,
                lon: -52.03125,
            },
            2,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                Some(OsmNode {
                    id: 3,
                    lat: 9.795677582829743,
                    lon: -1.7578125000000002,
                }),
                Some(OsmNode {
                    id: 4,
                    lat: -36.03133177633188,
                    lon: -65.21484375000001,
                }),
            ],
            OsmNode {
                id: 0,
                lat: -10.660607953624762,
                lon: -52.03125,
            },
            4,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                Some(OsmNode {
                    id: 3,
                    lat: 9.795677582829743,
                    lon: -1.7578125000000002,
                }),
                None,
            ],
            OsmNode {
                id: 0,
                lat: -10.660607953624762,
                lon: -52.03125,
            },
            3,
        ),
        (
            [
                Some(OsmNode {
                    id: 1,
                    lat: 57.16961885299059,
                    lon: 24.875192642211914,
                }),
                Some(OsmNode {
                    id: 2,
                    lat: 57.159484808175435,
                    lon: 24.877617359161377,
                }),
                Some(OsmNode {
                    id: 3,
                    lat: 9.795677582829743,
                    lon: -1.7578125000000002,
                }),
                Some(OsmNode {
                    id: 4,
                    lat: -36.03133177633188,
                    lon: -65.21484375000001,
                }),
            ],
            OsmNode {
                id: 0,
                lat: -28.92163128242129,
                lon: 144.14062500000003,
            },
            4,
        ),
    ];
    fn run_closest_test(test: ClosestTest) -> () {
        let (points, check_point, closest_id) = test;
        let mut map_data = MapDataGraph::new();
        for point in points {
            if let Some(point) = point {
                map_data.insert_node(point.clone());
            }
        }

        let map_data = set_graph_static(map_data);

        let closest = map_data.get_closest_to_coords(check_point.lat, check_point.lon);
        if let Some(closest) = closest {
            assert_eq!(closest.borrow().id, closest_id);
        } else {
            panic!("No points found");
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_0() {
            run_closest_test(CLOSEST_TESTS[0].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_1() {
            run_closest_test(CLOSEST_TESTS[1].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_2() {
            run_closest_test(CLOSEST_TESTS[2].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_3() {
            run_closest_test(CLOSEST_TESTS[3].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_4() {
            run_closest_test(CLOSEST_TESTS[4].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_5() {
            run_closest_test(CLOSEST_TESTS[5].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_6() {
            run_closest_test(CLOSEST_TESTS[6].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_7() {
            run_closest_test(CLOSEST_TESTS[7].clone());
        }
    }
}
