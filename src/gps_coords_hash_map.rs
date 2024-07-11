use std::{
    cell::{RefCell, RefMut},
    collections::{BTreeMap, HashMap},
    rc::Rc,
    u64,
};

use crate::gps_hash::{get_gps_coords_hash, HashOffset};

#[derive(Clone)]
pub struct MapDataNode {
    pub id: u64,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug)]
pub struct MapDataPoint {
    pub id: u64,
    pub lat: f64,
    pub lon: f64,
    pub part_of_ways: Vec<u64>,
    pub fork: bool,
}

#[derive(Clone)]
pub struct MapDataWay {
    pub id: u64,
    pub node_ids: Vec<u64>,
}

struct MapDataLine {
    id: String,
    way_id: u64,
    length_m: f64,
    direction_deg: f64,
}

type PointMap = BTreeMap<u64, Rc<RefCell<MapDataPoint>>>;

pub struct MapDataGraph {
    points: HashMap<u64, Rc<RefCell<MapDataPoint>>>,
    point_hashed_offset_none: PointMap,
    point_hashed_offset_lat: PointMap,
    nodes_hashed_offset_lon: PointMap,
    nodes_hashed_offset_lat_lon: PointMap,
    ways: HashMap<u64, MapDataWay>,
    lines: HashMap<String, MapDataLine>,
}

fn get_distance(from_lat: &f64, from_lon: &f64, to_lat: &f64, to_lon: &f64) -> f64 {
    // https://rust-lang-nursery.github.io/rust-cookbook/science/mathematics/trigonometry.html#distance-between-two-points-on-the-earth
    let earth_radius_kilometer = 6371.0;

    let from_lat_rad = from_lat.to_radians();
    let to_lat_rad = to_lat.to_radians();

    let delta_latitude = (from_lat - to_lat).to_radians();
    let delta_longitude = (from_lon - to_lon).to_radians();

    let central_angle_inner = (delta_latitude / 2.0).sin().powi(2)
        + from_lat_rad.cos() * to_lat_rad.cos() * (delta_longitude / 2.0).sin().powi(2);
    let central_angle = 2.0 * central_angle_inner.sqrt().asin();

    earth_radius_kilometer * central_angle
}

fn get_heading(from_lat: &f64, from_lon: &f64, to_lat: &f64, to_lon: &f64) -> f64 {
    // https://www.ridgesolutions.ie/index.php/2022/05/26/code-to-calculate-heading-bearing-from-two-gps-latitude-and-longitude/
    let from_lat_rad = from_lat.to_radians();
    let from_lon_rad = from_lon.to_radians();
    let to_lat_rad = to_lat.to_radians();
    let to_lon_rad = to_lon.to_radians();

    let delta_lon = to_lon_rad - from_lon_rad;
    let x = to_lat_rad.cos() * delta_lon.sin();
    let y = from_lat_rad.cos() * to_lat_rad.cos()
        - from_lat_rad.sin() * to_lat_rad.cos() * delta_lon.cos();

    let heading = x.atan2(y);
    let heading = heading.to_degrees();
    let heading = if heading < 0.0 {
        heading + 360.0
    } else {
        heading
    };

    heading
}

impl MapDataGraph {
    pub fn new() -> Self {
        Self {
            points: HashMap::new(),
            point_hashed_offset_none: BTreeMap::new(),
            point_hashed_offset_lat: BTreeMap::new(),
            nodes_hashed_offset_lon: BTreeMap::new(),
            nodes_hashed_offset_lat_lon: BTreeMap::new(),
            ways: HashMap::new(),
            lines: HashMap::new(),
        }
    }

    pub fn insert_node(&mut self, value: MapDataNode) -> () {
        let lat = value.lat.clone();
        let lon = value.lon.clone();
        let point = Rc::new(RefCell::new(MapDataPoint {
            id: value.id,
            lat: value.lat,
            lon: value.lon,
            part_of_ways: Vec::new(),
            fork: false,
        }));
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::None),
            Rc::clone(&point),
        );
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::Lat),
            Rc::clone(&point),
        );
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat.clone(), lon.clone(), HashOffset::Lon),
            Rc::clone(&point),
        );
        self.point_hashed_offset_none.insert(
            get_gps_coords_hash(lat, lon, HashOffset::LatLon),
            Rc::clone(&point),
        );
        let id = point.borrow().id.clone();
        self.points.insert(id, point);
    }

    pub fn insert_way(&mut self, value: MapDataWay) -> () {
        let prev_point: Option<MapDataNode> = None;
        for point_id in &value.node_ids {
            if let Some(point) = self.points.get(point_id) {
                let mut point: RefMut<'_, _> = point.borrow_mut();
                point.part_of_ways.push(point_id.clone());
                if point.part_of_ways.len() > 1 {
                    point.fork = true;
                }
                if let Some(prev_point) = &prev_point {
                    let line_id = format!("{}-{}-{}", &value.id, &prev_point.id, &point_id);
                    self.lines.insert(
                        line_id.clone(),
                        MapDataLine {
                            id: line_id,
                            way_id: value.id.clone(),
                            length_m: get_distance(
                                &prev_point.lat,
                                &prev_point.lon,
                                &point.lat,
                                &point.lon,
                            ),
                            direction_deg: get_heading(
                                &prev_point.lat,
                                &prev_point.lon,
                                &point.lat,
                                &point.lon,
                            ),
                        },
                    );
                }
            }
        }
        self.ways.insert(value.id.clone(), value);
    }

    pub fn get_adjacent_points(&self, node: MapDataNode) -> Vec<MapDataPoint> {
        let point = match self.points.get(&node.id) {
            None => return Vec::new(),
            Some(p) => p,
        };
        let points: Vec<_> = point
            .borrow()
            .part_of_ways
            .iter()
            .map(|w_id| self.ways.get(w_id))
            .filter_map(|w| {
                if let Some(w) = w {
                    let point_idx = w.node_ids.iter().position(|&p| p == node.id);
                    if let Some(point_idx) = point_idx {
                        let point_before = w.node_ids.get(point_idx - 1);
                        let point_after = w.node_ids.get(point_idx + 1);
                        return Some(
                            [point_before, point_after]
                                .iter()
                                .filter_map(|&p| p)
                                .map(|&p| p)
                                .collect::<Vec<u64>>(),
                        );
                    }
                }
                None
            })
            .flatten()
            .map(|p| self.points.get(&p))
            .filter_map(|p| {
                if let Some(p) = p {
                    return Some(p.borrow().clone());
                }
                None
            })
            .collect();

        points
    }

    pub fn get_closest_to_coords(&self, lat: f64, lon: f64) -> Option<MapDataNode> {
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
            let points: [Vec<Rc<RefCell<MapDataPoint>>>; 4] = [
                offset_none_points
                    .map(|(_, point)| Rc::clone(&point))
                    .collect(),
                offset_lat_points
                    .map(|(_, point)| Rc::clone(&point))
                    .collect(),
                offset_lon_points
                    .map(|(_, point)| Rc::clone(&point))
                    .collect(),
                offset_lat_lon_points
                    .map(|(_, point)| Rc::clone(&point))
                    .collect(),
            ];

            let points = points.concat();
            if !points.is_empty() || (from == 0 && to == u64::max_value()) {
                points.iter().for_each(|p| {
                    let id: u64 = p.borrow().id.clone();
                    grid_points.insert(id, Rc::clone(&p));
                });
                break;
            }
        }

        if grid_points.len() == 1 {
            let point = grid_points.values().next().map(|p| MapDataNode {
                id: p.borrow().id.clone(),
                lat: p.borrow().lat.clone(),
                lon: p.borrow().lon.clone(),
            });
            return point;
        }

        let mut points_with_dist: Vec<(u32, Rc<RefCell<MapDataPoint>>)> = grid_points
            .iter()
            .map(|(_, p)| {
                let distance = get_distance(&p.borrow().lat, &p.borrow().lon, &lat, &lon);
                (distance.round() as u32, Rc::clone(&p))
            })
            .collect();

        points_with_dist.sort_by(|(dist_a, _), (dist_b, _)| dist_a.cmp(dist_b));
        points_with_dist.get(0).map(|(_, p)| MapDataNode {
            id: p.borrow().id.clone(),
            lat: p.borrow().lat.clone(),
            lon: p.borrow().lon.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use core::panic;

    use super::*;

    #[test]
    fn closest_lookup() {
        let tests: Vec<(Vec<MapDataNode>, MapDataNode, u64)> = vec![
            (
                vec![MapDataNode {
                    id: 1,
                    lat: 57.1640,
                    lon: 24.8652,
                }],
                MapDataNode {
                    id: 0,
                    lat: 57.1670,
                    lon: 24.8658,
                },
                1,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.1640,
                        lon: 24.8652,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.1740,
                        lon: 24.8630,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: 57.1670,
                    lon: 24.8658,
                },
                1,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: 57.163429387682214,
                    lon: 24.87742424011231,
                },
                2,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: 57.193343289610794,
                    lon: 24.872531890869144,
                },
                1,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: -10.660607953624762,
                    lon: -52.03125,
                },
                1,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                    MapDataNode {
                        id: 3,
                        lat: 9.795677582829743,
                        lon: -1.7578125000000002,
                    },
                    MapDataNode {
                        id: 4,
                        lat: -36.03133177633188,
                        lon: -65.21484375000001,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: -10.660607953624762,
                    lon: -52.03125,
                },
                4,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                    MapDataNode {
                        id: 3,
                        lat: 9.795677582829743,
                        lon: -1.7578125000000002,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: -10.660607953624762,
                    lon: -52.03125,
                },
                3,
            ),
            (
                vec![
                    MapDataNode {
                        id: 1,
                        lat: 57.16961885299059,
                        lon: 24.875192642211914,
                    },
                    MapDataNode {
                        id: 2,
                        lat: 57.159484808175435,
                        lon: 24.877617359161377,
                    },
                    MapDataNode {
                        id: 3,
                        lat: 9.795677582829743,
                        lon: -1.7578125000000002,
                    },
                    MapDataNode {
                        id: 4,
                        lat: -36.03133177633188,
                        lon: -65.21484375000001,
                    },
                ],
                MapDataNode {
                    id: 0,
                    lat: -28.92163128242129,
                    lon: 144.14062500000003,
                },
                4,
            ),
        ];
        for (i, test) in tests.iter().enumerate() {
            let (points, check_point, closest_id) = test;
            let mut coords = MapDataGraph::new();
            for point in points {
                coords.insert_node(point.clone());
            }

            let closest = coords.get_closest_to_coords(check_point.lat, check_point.lon);
            if let Some(closest) = closest {
                eprintln!(
                    "{}: closest found id {} expected {}",
                    i, closest.id, closest_id
                );
                assert_eq!(closest.id, *closest_id);
            } else {
                panic!("No points found");
            }
        }
    }
}
