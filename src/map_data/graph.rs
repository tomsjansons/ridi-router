use std::{
    cmp::{Eq, Ordering},
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    hash::Hash,
    marker::PhantomData,
    sync::OnceLock,
    time::Instant,
};

use anyhow::Context;
use geo::{Distance, Haversine, Point};
use serde::{Deserialize, Serialize};
use tracing::trace;

#[cfg(feature = "debug-with-postgres")]
use crate::map_data::debug_writer::MapDebugWriter;
#[cfg(feature = "debug-with-postgres")]
use geo::{Coord, LineString};

use crate::{
    map_data::{
        osm::{OsmRelationMember, OsmRelationMemberRole, OsmRelationMemberType},
        rule::MapDataRule,
    },
    osm_data::{
        data_reader::{OsmDataReader, ALLOWED_HIGHWAY_VALUES},
        DataSource,
    },
    router::rules::{RouterRules, RulesTagValueAction},
};

use super::{
    line::{LineDirection, MapDataLine},
    osm::{OsmNode, OsmRelation, OsmWay},
    point::MapDataPoint,
    proximity::PointGrid,
    rule::MapDataRuleType,
    MapDataError,
};

pub static MAP_DATA_GRAPH: OnceLock<MapDataGraph> = OnceLock::new();

#[derive(PartialEq, Eq, Hash, Debug, Clone, Serialize, Deserialize)]
struct ElementTagValueRef {
    pub tag_value_pos: u32,
}
impl ElementTagValueRef {
    pub fn none() -> Self {
        Self { tag_value_pos: 0 }
    }
    pub fn some(tag_idx: u32) -> Self {
        Self {
            tag_value_pos: tag_idx + 1,
        }
    }
    pub fn borrow(&self) -> Option<&smartstring::alias::String> {
        let idx = if self.tag_value_pos == 0 {
            return None;
        } else {
            self.tag_value_pos - 1
        };
        Some(&MapDataGraph::get().tags.tag_values[idx as usize])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementTagSetRef {
    tag_set_idx: u32,
}

impl ElementTagSetRef {
    pub fn borrow(&self) -> &ElementTagSet {
        &MapDataGraph::get().tags.tag_sets[self.tag_set_idx as usize]
    }
    pub fn new(idx: u32) -> Self {
        Self { tag_set_idx: idx }
    }
}

#[derive(PartialEq, Eq, Hash, Debug, Clone, Serialize, Deserialize)]
pub struct ElementTagSet {
    name: ElementTagValueRef,
    hw_ref: ElementTagValueRef,
    highway: ElementTagValueRef,
    surface: ElementTagValueRef,
    smoothness: ElementTagValueRef,
}

impl ElementTagSet {
    pub fn name(&self) -> Option<&smartstring::alias::String> {
        self.name.borrow()
    }
    pub fn hw_ref(&self) -> Option<&smartstring::alias::String> {
        self.hw_ref.borrow()
    }
    pub fn highway(&self) -> Option<&smartstring::alias::String> {
        self.highway.borrow()
    }
    pub fn surface(&self) -> Option<&smartstring::alias::String> {
        self.surface.borrow()
    }
    pub fn smoothness(&self) -> Option<&smartstring::alias::String> {
        self.smoothness.borrow()
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct ElementTags {
    pub tag_values: Vec<smartstring::alias::String>,
    pub tag_sets: Vec<ElementTagSet>,
    tag_map: HashMap<smartstring::alias::String, u32>,
    tag_set_map: HashMap<ElementTagSet, u32>,
}

impl ElementTags {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> (usize, usize) {
        (self.tag_values.len(), self.tag_sets.len())
    }
    pub fn clear_maps(&mut self) {
        self.tag_set_map = HashMap::new();
        self.tag_map = HashMap::new();
    }
    pub fn get_or_create(
        &mut self,
        name: Option<&String>,
        hw_ref: Option<&String>,
        highway: Option<&String>,
        surface: Option<&String>,
        smoothness: Option<&String>,
    ) -> ElementTagSetRef {
        let name_ref = self.get_tag_value_ref(name);
        let hw_ref_ref = self.get_tag_value_ref(hw_ref);
        let highway_ref = self.get_tag_value_ref(highway);
        let surface_ref = self.get_tag_value_ref(surface);
        let smoothness_ref = self.get_tag_value_ref(smoothness);

        let tag_set = ElementTagSet {
            name: name_ref,
            hw_ref: hw_ref_ref,
            highway: highway_ref,
            surface: surface_ref,
            smoothness: smoothness_ref,
        };
        let idx = match self.tag_set_map.get(&tag_set) {
            Some(i) => *i,
            None => {
                let new_idx = self.tag_sets.len() as u32;
                self.tag_set_map.insert(tag_set.clone(), new_idx);
                self.tag_sets.push(tag_set);
                new_idx
            }
        };
        ElementTagSetRef::new(idx)
    }
    fn get_tag_value_ref(&mut self, value: Option<&String>) -> ElementTagValueRef {
        match value {
            None => ElementTagValueRef::none(),
            Some(v) => {
                let v = if v.ends_with("_link") {
                    v.replace("_link", "")
                } else {
                    v.to_string()
                };
                let idx = match self.tag_map.get(&smartstring::alias::String::from(&v)) {
                    Some(i) => *i,
                    None => {
                        let new_idx = self.tag_values.len() as u32;
                        self.tag_values.push(smartstring::alias::String::from(&v));
                        self.tag_map
                            .insert(smartstring::alias::String::from(&v), new_idx);
                        new_idx
                    }
                };
                ElementTagValueRef::some(idx)
            }
        }
    }
}

pub trait MapDataElement: Debug + Display {
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

#[derive(Serialize, Deserialize)]
pub struct MapDataElementRef<T: MapDataElement> {
    idx: usize,
    _marker: PhantomData<T>,
}

impl<T: MapDataElement + 'static> Display for MapDataElementRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ref({}, idx: {})", self.borrow(), self.idx)
    }
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
        Debug::fmt(&self.borrow(), f)
    }
}

pub type MapDataLineRef = MapDataElementRef<MapDataLine>;
pub type MapDataPointRef = MapDataElementRef<MapDataPoint>;

#[derive(Serialize, Deserialize)]
pub struct MapDataGraph {
    points: Vec<MapDataPoint>,
    points_map: HashMap<u64, usize>,
    point_grid: PointGrid<MapDataPointRef>,
    ways_lines: HashMap<u64, Vec<MapDataLineRef>>,
    lines: Vec<MapDataLine>,
    tags: ElementTags,
}

#[derive(Default)]
pub struct MapDataGraphPacked {
    pub points: Vec<u8>,
    pub lines: Vec<u8>,
    pub tags: Vec<u8>,
    pub point_grid: Vec<u8>,
}

impl MapDataGraph {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            points_map: HashMap::new(),
            point_grid: PointGrid::new(),
            ways_lines: HashMap::new(),
            lines: Vec::new(),
            tags: ElementTags::new(),
        }
    }

    pub fn pack(&self) -> anyhow::Result<MapDataGraphPacked> {
        let pack_start = Instant::now();

        let mut packed = MapDataGraphPacked::default();

        trace!("points len {}", self.points.len());
        trace!("proximity_lookup len {}", self.point_grid.len(),);
        trace!("lines len {}", self.lines.len());
        trace!("tags len {:?}", self.tags.len());

        let mut points: Option<anyhow::Result<Vec<u8>>> = None;
        let mut point_grid: Option<anyhow::Result<Vec<u8>>> = None;
        let mut lines: Option<anyhow::Result<Vec<u8>>> = None;
        let mut tags: Option<anyhow::Result<Vec<u8>>> = None;

        rayon::scope(|scope| {
            scope.spawn(|_| {
                points =
                    Some(bincode::serialize(&self.points).context("Failed to serialize points"));
            });
            scope.spawn(|_| {
                point_grid = Some(
                    bincode::serialize(&self.point_grid).context("Failed to serialize point grid"),
                );
            });
            scope.spawn(|_| {
                lines = Some(bincode::serialize(&self.lines).context("Failed to serialize lines"));
            });
            scope.spawn(|_| {
                tags = Some(bincode::serialize(&self.tags).context("could not serialize tags"));
            });
        });
        packed.points = points.context("Points missing")??;
        packed.point_grid = point_grid.context("Points grid missing")??;
        packed.lines = lines.context("Lines missing")??;
        packed.tags = tags.context("Tags missing")??;

        trace!("points len {}, {}", self.points.len(), packed.points.len());
        trace!(
            "point_grid len {}, {}",
            self.point_grid.len(),
            packed.point_grid.len()
        );
        trace!("lines len {} {}", self.lines.len(), packed.lines.len());
        trace!("tags len {:?} {}", self.tags.len(), packed.tags.len());

        let pack_end = pack_start.elapsed();
        trace!("pack took {}s", pack_end.as_secs());

        Ok(packed)
    }

    #[cfg(test)]
    pub fn test_get_point_ref_by_id(&self, id: &u64) -> Option<MapDataPointRef> {
        self.get_point_ref_by_id(id)
    }

    fn get_point_ref_by_id(&self, id: &u64) -> Option<MapDataPointRef> {
        self.points_map.get(id).map(|i| MapDataElementRef::new(*i))
    }

    pub fn insert_node(&mut self, value: OsmNode) {
        let point = MapDataPoint {
            id: value.id,
            lat: value.lat as f32,
            lon: value.lon as f32,
            lines: Vec::new(),
            rules: Vec::new(),
            residential_in_proximity: value.residential_in_proximity,
            nogo_area: value.nogo_area,
        };
        self.add_point(point.clone());
    }

    #[cfg(feature = "debug-with-postgres")]
    fn write_debug(&self) -> () {
        let mut debug_writer = MapDebugWriter::new();
        for line in &self.lines {
            let point_1 = &self.points[line.points.0.idx];
            let point_2 = &self.points[line.points.1.idx];
            if point_1.residential_in_proximity || point_2.residential_in_proximity {
                debug_writer.write_line_residential_close(&LineString::new(vec![
                    Coord {
                        x: point_1.lon as f64,
                        y: point_1.lat as f64,
                    },
                    Coord {
                        x: point_2.lon as f64,
                        y: point_2.lat as f64,
                    },
                ]));
            } else {
                debug_writer.write_line_residential_not_close(&LineString::new(vec![
                    Coord {
                        x: point_1.lon as f64,
                        y: point_1.lat as f64,
                    },
                    Coord {
                        x: point_2.lon as f64,
                        y: point_2.lat as f64,
                    },
                ]));
            }
        }
        debug_writer.flush();
    }

    pub fn generate_point_hashes(&mut self) {
        for point in self.points.iter().filter(|p| !p.lines.is_empty()) {
            let point_idx = self
                .points_map
                .get(&point.id)
                .expect("Point must exist in the points map, something went very wrong");
            let point_ref = MapDataElementRef::new(*point_idx);
            self.point_grid.insert(point.lat, point.lon, &point_ref);
        }

        #[cfg(feature = "debug-with-postgres")]
        self.write_debug();

        if !cfg!(test) {
            self.points_map = HashMap::new();
            self.ways_lines = HashMap::new();
            self.tags.clear_maps();
        }
    }

    fn get_mut_point_by_idx(&mut self, idx: usize) -> &mut MapDataPoint {
        &mut self.points[idx]
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
            let motorcycle = match tags.get("motorcycle") {
                Some(v) => v == "yes",
                None => false,
            };

            if let Some(highway) = tags.get("highway") {
                return ALLOWED_HIGHWAY_VALUES.contains(&highway.as_str())
                    && (highway != "path" || (highway == "path" && motorcycle));
            }
        }
        false
    }

    pub fn insert_way(&mut self, osm_way: OsmWay) -> Result<(), MapDataError> {
        if !self.way_is_ok(&osm_way) {
            return Ok(());
        }
        let mut prev_point_ref: Option<MapDataPointRef> = None;

        let mut way_line_refs = Vec::new();
        for point_id in &osm_way.point_ids {
            if let Some(point_ref) = self.get_point_ref_by_id(point_id) {
                if let Some(prev_point_ref) = prev_point_ref {
                    let tag_name = osm_way.tags.as_ref().and_then(|t| t.get("name"));
                    let tag_ref = osm_way.tags.as_ref().and_then(|t| t.get("ref"));
                    let tag_surface = osm_way.tags.as_ref().and_then(|t| t.get("surface"));
                    let tag_smoothness = osm_way.tags.as_ref().and_then(|t| t.get("smoothness"));
                    let tag_highway = osm_way.tags.as_ref().and_then(|t| t.get("highway"));
                    let line = MapDataLine {
                        points: (prev_point_ref.clone(), point_ref.clone()),
                        direction: if osm_way.is_roundabout() {
                            LineDirection::Roundabout
                        } else if osm_way.is_one_way() {
                            LineDirection::OneWay
                        } else {
                            LineDirection::BothWays
                        },
                        tags: self.tags.get_or_create(
                            tag_name,
                            tag_ref,
                            tag_highway,
                            tag_surface,
                            tag_smoothness,
                        ),
                    };
                    let line_idx = self.add_line(line);
                    let line_ref = MapDataLineRef::new(line_idx);
                    way_line_refs.push(line_ref.clone());

                    let point_mut = self.get_mut_point_by_idx(point_ref.idx);
                    point_mut.lines.push(line_ref.clone());

                    let prev_point_mut = self.get_mut_point_by_idx(prev_point_ref.idx);
                    prev_point_mut.lines.push(line_ref);
                }
                prev_point_ref = Some(point_ref);
            } else {
                return Err(MapDataError::MissingPoint {
                    point_id: *point_id,
                });
            }
        }
        self.ways_lines.insert(osm_way.id, way_line_refs);

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
        let rule_type = match restriction.split(" ").collect::<Vec<_>>().first() {
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
            fn get_lines_from_way_ids(
                graph: &MapDataGraph,
                members: &Vec<OsmRelationMember>,
                role: OsmRelationMemberRole,
            ) -> Vec<MapDataLineRef> {
                members
                    .iter()
                    .filter_map(|member| {
                        if member.role == role {
                            return Some(member.member_ref);
                        }
                        None
                    })
                    .filter_map(|w_id| graph.ways_lines.get(&w_id))
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>()
            }
            let from_lines =
                get_lines_from_way_ids(self, &relation.members, OsmRelationMemberRole::From);
            let to_lines =
                get_lines_from_way_ids(self, &relation.members, OsmRelationMemberRole::To);

            if from_lines.is_empty() || to_lines.is_empty() {
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

    fn get_avoid_rules(rules: &RouterRules) -> HashSet<String> {
        let mut avoid_tags = HashSet::new();

        if let Some(ref hw) = rules.highway {
            hw.iter().for_each(|(tag_value, tag_action)| {
                if tag_action == &RulesTagValueAction::Avoid {
                    avoid_tags.insert(format!("highway:{tag_value}"));
                }
            });
        }
        if let Some(ref surface) = rules.surface {
            surface.iter().for_each(|(tag_value, tag_action)| {
                if tag_action == &RulesTagValueAction::Avoid {
                    avoid_tags.insert(format!("surface:{tag_value}"));
                }
            });
        }
        if let Some(ref smoothness) = rules.smoothness {
            smoothness.iter().for_each(|(tag_value, tag_action)| {
                if tag_action == &RulesTagValueAction::Avoid {
                    avoid_tags.insert(format!("smoothness:{tag_value}"));
                }
            });
        }

        avoid_tags
    }

    pub fn get_closest_to_coords(
        &self,
        lat: f32,
        lon: f32,
        rules: &RouterRules,
        avoid_proximity_to_residential: bool,
    ) -> Option<MapDataPointRef> {
        let closest_points = self.point_grid.find_closest_point_refs(lat, lon, 20);
        let closest_points = match closest_points {
            Some(p) => p,
            None => return None,
        };

        let avoid_tags = Self::get_avoid_rules(rules);

        let mut distances = closest_points
            .iter()
            .filter(|p| {
                if avoid_proximity_to_residential && p.borrow().residential_in_proximity {
                    return false;
                }
                let lines = p
                    .borrow()
                    .lines
                    .iter()
                    .map(|line| line.borrow())
                    .collect::<Vec<_>>();
                let mut hws = lines.iter().filter_map(|line| {
                    line.tags
                        .borrow()
                        .highway()
                        .map(|hw| format!("highway:{hw}"))
                });
                let mut surfaces = lines.iter().filter_map(|line| {
                    line.tags
                        .borrow()
                        .surface()
                        .map(|surface| format!("surface:{surface}"))
                });
                let mut smoothnesses = lines.iter().filter_map(|line| {
                    line.tags
                        .borrow()
                        .smoothness()
                        .map(|sm| format!("smoothness:{sm}"))
                });

                if hws.any(|tag| avoid_tags.contains(&tag))
                    || surfaces.any(|tag| avoid_tags.contains(&tag))
                    || smoothnesses.any(|tag| avoid_tags.contains(&tag))
                {
                    return false;
                }
                true
            })
            .map(|p| {
                let point = &self.points[p.idx];
                let geo_point = Point::new(point.lon, point.lat);
                let geo_lookup_point = Point::new(lon, lat);
                (*p, Haversine.distance(geo_point, geo_lookup_point))
            })
            .collect::<Vec<(&MapDataPointRef, f32)>>();

        distances.sort_by(|el1, el2| {
            if el1.1 > el2.1 {
                Ordering::Greater
            } else if el1.1 < el2.1 {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });

        distances.first().map(|v| v.0.clone())
    }
    #[tracing::instrument(skip(packed))]
    pub fn unpack(packed: MapDataGraphPacked) -> anyhow::Result<&'static MapDataGraph> {
        let mut points: Option<anyhow::Result<Vec<MapDataPoint>>> = None;
        let points_map = HashMap::new();
        let mut point_grid: Option<anyhow::Result<PointGrid<MapDataPointRef>>> = None;
        let ways_lines = HashMap::new();
        let mut lines: Option<anyhow::Result<Vec<MapDataLine>>> = None;
        let mut tags: Option<anyhow::Result<ElementTags>> = None;

        let unpack_start = Instant::now();
        rayon::scope(|scope| {
            scope.spawn(|_| {
                let start = Instant::now();
                points = Some(
                    bincode::deserialize(&packed.points[..])
                        .context("could not deserialize points"),
                );
                let dur = start.elapsed();
                trace!("points {}s", dur.as_secs());
            });
            scope.spawn(|_| {
                let start = Instant::now();
                point_grid = Some(
                    bincode::deserialize(&packed.point_grid[..])
                        .context("could not deserialize points"),
                );
                let dur = start.elapsed();
                trace!("point_grid {}s", dur.as_secs());
            });
            scope.spawn(|_| {
                let start = Instant::now();
                lines = Some(
                    bincode::deserialize(&packed.lines[..]).context("could not deserialize lines"),
                );
                let dur = start.elapsed();
                trace!("lines {}s", dur.as_secs());
            });
            scope.spawn(|_| {
                let start = Instant::now();
                tags = Some(
                    bincode::deserialize(&packed.tags[..]).context("could not deserialize tags"),
                );
                let dur = start.elapsed();
                trace!("tags {}s", dur.as_secs());
            });
        });
        let unpack_duration = unpack_start.elapsed();
        trace!(time = ?unpack_duration, "Unpack finished");

        let points = points.context("Points missing")??;
        let point_grid = point_grid.context("Point grid missing")??;
        let lines = lines.context("Lines missing")??;
        let tags = tags.context("Tags missing")??;

        Ok(MAP_DATA_GRAPH.get_or_init(|| MapDataGraph {
            points,
            points_map,
            point_grid,
            lines,
            ways_lines,
            tags,
        }))
    }

    fn get_or_init(data_source: Option<&DataSource>) -> &'static MapDataGraph {
        MAP_DATA_GRAPH.get_or_init(|| {
            let data_source = data_source.expect("data source must passed in when calling init");
            let data_reader = OsmDataReader::new(data_source.clone());

            // will panic on purpose as it means it's been incorrectly called
            // it is a fatal error can't be recovered from
            data_reader.read_data().unwrap()
        })
    }
    #[tracing::instrument]
    pub fn init(data_source: &DataSource) {
        MapDataGraph::get_or_init(Some(data_source));
    }
    pub fn get() -> &'static MapDataGraph {
        MapDataGraph::get_or_init(None) // we've already initialized the graph
    }
}

#[cfg(test)]
mod tests {
    use core::panic;
    use std::{collections::HashSet, u8};

    use rusty_fork::rusty_fork_test;
    use tracing::info;

    use crate::{
        router::rules::{BasicRules, GenerationRules},
        test_utils::{graph_from_test_dataset, set_graph_static, test_dataset_1},
    };

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

        assert!(map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "proposed".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "cycleway".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "hhhighway".to_string(),
                "primary".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "steps".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "pedestrian".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([("highway".to_string(), "path".to_string())])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "service".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([(
                "highway".to_string(),
                "footway".to_string(),
            )])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([("highway".to_string(), "omg".to_string())])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
            ])),
        };

        assert!(map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "no".to_string()),
            ])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "private".to_string()),
            ])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("service".to_string(), "yes".to_string()),
            ])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "yes".to_string()),
            ])),
        };

        assert!(map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "no".to_string()),
            ])),
        };

        assert!(!map_data.way_is_ok(&osm_way));

        let osm_way = OsmWay {
            id: 1,
            point_ids: Vec::new(),
            tags: Some(HashMap::from([
                ("highway".to_string(), "primary".to_string()),
                ("motor_vehicle".to_string(), "yes".to_string()),
                ("access".to_string(), "private".to_string()),
            ])),
        };

        assert!(!map_data.way_is_ok(&osm_way));
    }

    #[derive(Debug)]
    struct PointTest {
        lat: f32,
        lon: f32,
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
                    .unwrap_or_else(|| panic!("point {} must exist", id));
                let point = point.borrow();
                info!("point {:#?}", point);
                info!("test {:#?}", test);
                point.lat == test.lat
                    && point.lon == test.lon
                    && point.lines.len() == test.lines.len()
                    && point.lines.iter().enumerate().all(|(idx, l)| {
                        let test_line_id = test
                            .lines
                            .get(idx)
                            .unwrap_or_else(|| panic!("{}: line at idx {} must exist", id, idx));
                        l.borrow().line_id() == *test_line_id
                    })
                    && point.is_junction() == test.junction
            }
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));
            assert!(point_is_ok(
                map_data,
                &1,
                PointTest {
                    lat: 1.0,
                    lon: 1.0,
                    lines: vec!["1-2"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &2,
                PointTest {
                    lat: 2.0,
                    lon: 2.0,
                    lines: vec!["1-2", "2-3"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &3,
                PointTest {
                    lat: 3.0,
                    lon: 3.0,
                    lines: vec!["2-3", "3-4", "5-3", "3-6"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                map_data,
                &4,
                PointTest {
                    lat: 4.0,
                    lon: 4.0,
                    lines: vec!["3-4", "4-8"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &5,
                PointTest {
                    lat: 5.0,
                    lon: 5.0,
                    lines: vec!["5-3"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &6,
                PointTest {
                    lat: 6.0,
                    lon: 6.0,
                    lines: vec!["3-6", "6-7", "6-8"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                map_data,
                &7,
                PointTest {
                    lat: 7.0,
                    lon: 7.0,
                    lines: vec!["6-7"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &8,
                PointTest {
                    lat: 8.0,
                    lon: 8.0,
                    lines: vec!["4-8", "8-9", "6-8"],
                    junction: true
                }
            ));
            assert!(point_is_ok(
                map_data,
                &9,
                PointTest {
                    lat: 9.0,
                    lon: 9.0,
                    lines: vec!["8-9"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &11,
                PointTest {
                    lat: 11.0,
                    lon: 11.0,
                    lines: vec!["11-12"],
                    junction: false
                }
            ));
            assert!(point_is_ok(
                map_data,
                &12,
                PointTest {
                    lat: 12.0,
                    lon: 12.0,
                    lines: vec!["11-12"],
                    junction: false
                }
            ));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn check_line_consistency() {
            fn line_is_ok(
                map_data: &MapDataGraph,
                id: &str,
                test_points: (u64, u64),
            ) -> bool {
                let line = map_data
                    .lines
                    .iter()
                    .find(|l| l.line_id() == *id)
                    .unwrap_or_else(|| panic!("line {} must exist", id));
                info!("line {:#?}", line);
                info!("test {:#?}", test_points);
                     line.points.0.borrow().id == test_points.0
                    && line.points.1.borrow().id == test_points.1
            }
            let map_data = set_graph_static(graph_from_test_dataset(test_dataset_1()));
            assert!(line_is_ok(map_data, "1-2", (1, 2)));
            assert!(line_is_ok(map_data, "2-3", (2, 3)));
            assert!(line_is_ok(map_data, "3-4", (3, 4)));
            assert!(line_is_ok(map_data, "5-3", (5, 3)));
            assert!(line_is_ok(map_data, "3-6", (3, 6)));
            assert!(line_is_ok(map_data, "6-7", (6, 7)));
            assert!(line_is_ok(map_data, "4-8", (4, 8)));
            assert!(line_is_ok(map_data, "8-9", (8, 9)));
            assert!(line_is_ok(map_data, "6-8", (6, 8)));
            assert!(line_is_ok(map_data, "11-12", (11, 12)));
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
            if res.is_ok() {
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
                assert!((p.1.borrow().id == 3 && p.1.borrow().is_junction()) || p.1.borrow().id != 3)
            });

            let point = map_data.get_point_ref_by_id(&3).unwrap();
            let points = map_data.get_adjacent(point);
            let non_junctions = [2, 5, 4];
            points.iter().for_each(|p| {
                assert!(
                    ((non_junctions.contains(&p.1.borrow().id) && !p.1.borrow().is_junction())
                        || !non_junctions.contains(&p.1.borrow().id))
                )
            });
            points.iter().for_each(|p| {
                assert!((p.1.borrow().id == 6 && p.1.borrow().is_junction()) || p.1.borrow().id != 6)
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
                    vec![(String::from("1-2"), 1), (String::from("2-3"), 3)],
                ),
                (
                    2,
                    MapDataGraph::get().get_point_ref_by_id(&3).unwrap(),
                    vec![
                        (String::from("5-3"), 5),
                        (String::from("6-3"), 6),
                        (String::from("2-3"), 2),
                        (String::from("4-3"), 4),
                    ],
                ),
                (
                    3,
                    MapDataGraph::get().get_point_ref_by_id(&1).unwrap(),
                    vec![(String::from("1-2"), 2)],
                ),
            ];

            for test in tests {
                let (_test_id, point, expected_result) = test;
                let adj_elements = map_data.get_adjacent(point);
                assert_eq!(adj_elements.len(), expected_result.len());
                for (adj_line, adj_point) in &adj_elements {
                    let adj_match = expected_result.iter().find(|&(line_id, point_id)| {
                        line_id.split("-").collect::<HashSet<_>>()
                            == adj_line.borrow().line_id().split("-").collect::<HashSet<_>>()
                            && point_id == &adj_point.borrow().id
                    });
                    assert!(adj_match.is_some());
                }
            }
        }
    }

    type ClosestTest = (Vec<OsmNode>, Vec<OsmWay>, Option<RouterRules>, OsmNode, u64);

    fn run_closest_test(test: ClosestTest) {
        let (points, ways, rules, check_point, closest_id) = test;
        let mut map_data = MapDataGraph::new();
        for point in &points {
            map_data.insert_node(point.clone());
        }
        for point in points {
            if !ways.iter().any(|w| w.point_ids.contains(&point.id)) {
                map_data
                    .insert_way(OsmWay {
                        id: point.id,
                        tags: Some(HashMap::from([(
                            "highway".to_string(),
                            "primary".to_string(),
                        )])),
                        point_ids: vec![point.id, point.id],
                    })
                    .expect("failed to insert dummy way");
            }
        }
        for way in ways {
            map_data.insert_way(way).expect("failed to insert way");
        }

        map_data.generate_point_hashes();

        let map_data = set_graph_static(map_data);

        let closest = map_data.get_closest_to_coords(
            check_point.lat as f32,
            check_point.lon as f32,
            &rules.map_or(
                RouterRules {
                    ..Default::default()
                },
                |r| r,
            ),
            false,
        );
        if let Some(closest) = closest {
            assert_eq!(closest.borrow().id, closest_id);
        } else {
            panic!("No points found");
        }
    }
    fn get_closest_tests() -> [ClosestTest; 6] {
        [
            (
                vec![
                    // 0
                    OsmNode {
                        id: 1,
                        lat: 57.1640,
                        lon: 24.8652,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![],
                None,
                OsmNode {
                    id: 0,
                    lat: 57.1670,
                    lon: 24.8658,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                1,
            ),
            (
                vec![
                    // 1
                    OsmNode {
                        id: 1,
                        lat: 57.1740,
                        lon: 24.8630,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        id: 2,
                        lat: 57.1640,
                        lon: 24.8652,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![],
                None,
                OsmNode {
                    id: 0,
                    lat: 57.1670,
                    lon: 24.8658,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                2,
            ),
            (
                vec![
                    // 2
                    OsmNode {
                        // 701.26 meters
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 525.74 meters
                        id: 2,
                        lat: 57.168,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 438.77 meters
                        id: 3,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![],
                None,
                OsmNode {
                    id: 0,
                    lat: 57.163429387682214,
                    lon: 24.87742424011231,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                3,
            ),
            (
                vec![
                    // 3
                    OsmNode {
                        // 2642.91 meters
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 3777.35 meters
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![],
                None,
                OsmNode {
                    id: 0,
                    lat: 57.193343289610794,
                    lon: 24.872531890869144,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                1,
            ),
            (
                vec![
                    // 4
                    OsmNode {
                        // 2642.91 meters
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 3777.35 meters
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![],
                None,
                OsmNode {
                    id: 0,
                    lat: 57.193343289610794,
                    lon: 24.872531890869144,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                1,
            ),
            (
                vec![
                    // 5
                    OsmNode {
                        // 701.26 meters
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 525.74 meters
                        id: 2,
                        lat: 57.168,
                        lon: 24.875192642211914,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                    OsmNode {
                        // 438.77 meters
                        id: 3,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                        residential_in_proximity: false,
                        nogo_area: false,
                    },
                ],
                vec![OsmWay {
                    id: 33,
                    point_ids: vec![3],
                    tags: Some(HashMap::from([(
                        "highway".to_string(),
                        "trunk".to_string(),
                    )])),
                }],
                Some(RouterRules {
                    basic: BasicRules::default(),
                    highway: Some(HashMap::from([(
                        "trunk".to_string(),
                        RulesTagValueAction::Avoid,
                    )])),
                    surface: None,
                    smoothness: None,
                    generation: GenerationRules::default(),
                }),
                OsmNode {
                    id: 0,
                    lat: 57.163429387682214,
                    lon: 24.87742424011231,
                    residential_in_proximity: false,
                    nogo_area: false,
                },
                2,
            ),
        ]
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_0() {
            let tests = get_closest_tests();
            run_closest_test(tests[0].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_1() {
            let tests = get_closest_tests();
            run_closest_test(tests[1].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_2() {
            let tests = get_closest_tests();
            run_closest_test(tests[2].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_3() {
            let tests = get_closest_tests();
            run_closest_test(tests[3].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_4() {
            let tests = get_closest_tests();
            run_closest_test(tests[4].clone());
        }
    }
    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn closest_lookup_5() {
            let tests = get_closest_tests();
            run_closest_test(tests[5].clone());
        }
    }
}
