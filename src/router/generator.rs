use std::collections::HashMap;

use crate::{
    map_data::graph::{MapDataGraph, MapDataPointRef},
    router::{clustering::Clustering, rules::RouterRules},
};
use geo::{Bearing, Destination, GeoNum, Haversine, Point};
use rayon::prelude::*;
use tracing::{info, trace};

use super::{
    itinerary::Itinerary,
    navigator::{NavigationResult, Navigator},
    route::{Route, RouteStats},
    weights::{
        weight_check_distance_to_next, weight_heading, weight_no_loops, weight_no_sharp_turns,
        weight_no_short_detours, weight_prefer_same_road, weight_progress_speed,
        weight_rules_highway, weight_rules_smoothness, weight_rules_surface,
    },
};

const START_FINISH_VARIATION_DISTANCES: [f32; 3] = [10000., 20000., 30000.];
const START_FINISH_VARIATION_DEGREES: [f32; 8] = [0., 45., 90., 135., 180., 225., 270., 315.];
const ROUND_TRIP_TIP_DISTANCE_RATIOS: [f32; 10] =
    [0.5, 0.45, 0.4, 0.35, 0.3, 0.25, 0.2, 0.15, 0.1, 0.05];
const ROUND_TRIP_SIDES_DISTANCE_RATIOS: [f32; 5] = [0.9, 0.7, 0.5, 0.2, 0.1];
const ROUND_TRIP_BEARING_VARIATION: [f32; 5] = [-20., -10., 0., 10., 20.];

#[derive(Debug, Clone)]
pub struct RouteWithStats {
    pub stats: RouteStats,
    pub route: Route,
}

pub struct Generator {
    start: MapDataPointRef,
    finish: MapDataPointRef,
    round_trip: Option<(f32, u32)>,
    rules: RouterRules,
}

impl Generator {
    pub fn new(
        start: MapDataPointRef,
        finish: MapDataPointRef,
        round_trip: Option<(f32, u32)>,
        rules: RouterRules,
    ) -> Self {
        Self {
            start,
            finish,
            round_trip,
            rules,
        }
    }

    fn create_waypoints_around(&self, point: &MapDataPointRef) -> Vec<MapDataPointRef> {
        let point_geo = Point::new(point.borrow().lon, point.borrow().lat);
        START_FINISH_VARIATION_DEGREES
            .iter()
            .map(|bearing| {
                START_FINISH_VARIATION_DISTANCES
                    .iter()
                    .map(|distance| {
                        let wp_geo = Haversine::destination(point_geo, *bearing, *distance);
                        let wp = MapDataGraph::get().get_closest_to_coords(wp_geo.y(), wp_geo.x());
                        wp
                    })
                    .filter_map(|maybe_wp| maybe_wp)
            })
            .flatten()
            .collect()
    }

    #[tracing::instrument(skip(self))]
    fn generate_itineraries(&self) -> Vec<Itinerary> {
        if let Some(round_trip) = self.round_trip {
            let start_geo = Point::new(self.start.borrow().lon, self.start.borrow().lat);

            return ROUND_TRIP_SIDES_DISTANCE_RATIOS
                .iter()
                .map(|side_left_ratio| {
                    let bearing = round_trip.0;
                    ROUND_TRIP_TIP_DISTANCE_RATIOS
                        .iter()
                        .map(|tip_ratio| {
                            ROUND_TRIP_BEARING_VARIATION
                                .iter()
                                .filter_map(|bearing_variation| {
                                    let mut tot_dist = round_trip.1 as f32;
                                    let tip_geo = Haversine::destination(
                                        start_geo.clone(),
                                        bearing + bearing_variation,
                                        tot_dist * tip_ratio,
                                    );
                                    tot_dist -= tot_dist * tip_ratio;
                                    let tip_point = match MapDataGraph::get()
                                        .get_closest_to_coords(tip_geo.y(), tip_geo.x())
                                    {
                                        None => return None,
                                        Some(p) => p,
                                    };

                                    let side_left_geo = Haversine::destination(
                                        start_geo.clone(),
                                        bearing + bearing_variation - 45.,
                                        tot_dist * side_left_ratio,
                                    );
                                    tot_dist -= tot_dist * side_left_ratio;

                                    let side_left_point = match MapDataGraph::get()
                                        .get_closest_to_coords(side_left_geo.y(), side_left_geo.x())
                                    {
                                        None => return None,
                                        Some(p) => p,
                                    };

                                    let side_right_geo = Haversine::destination(
                                        start_geo.clone(),
                                        bearing + bearing_variation - 45.,
                                        tot_dist,
                                    );

                                    let side_right_point = match MapDataGraph::get()
                                        .get_closest_to_coords(
                                            side_right_geo.y(),
                                            side_right_geo.x(),
                                        ) {
                                        None => return None,
                                        Some(p) => p,
                                    };

                                    Some(Itinerary::new(
                                        self.start.clone(),
                                        self.finish.clone(),
                                        vec![side_left_point, tip_point, side_right_point],
                                        5.,
                                    ))
                                })
                                .collect::<Vec<_>>()
                        })
                        .flatten()
                        .collect::<Vec<_>>()
                })
                .flatten()
                .collect();
        }
        let from_waypoints = self.create_waypoints_around(&self.start);
        let to_waypoints = self.create_waypoints_around(&self.finish);
        let mut itineraries = vec![Itinerary::new(
            self.start.clone(),
            self.finish.clone(),
            Vec::new(),
            10.,
        )];

        from_waypoints.iter().for_each(|from_wp| {
            to_waypoints.iter().for_each(|to_wp| {
                itineraries.push(Itinerary::new(
                    self.start.clone(),
                    self.finish.clone(),
                    vec![from_wp.clone(), to_wp.clone()],
                    1000.,
                ))
            })
        });
        itineraries
    }

    #[tracing::instrument(skip(self))]
    pub fn generate_routes(self) -> Vec<RouteWithStats> {
        let itineraries = self.generate_itineraries();
        info!("Created {} itineraries", itineraries.len());
        let routes = itineraries
            .into_par_iter()
            .map(|itinerary| {
                Navigator::new(
                    itinerary,
                    self.rules.clone(),
                    vec![
                        weight_no_sharp_turns,
                        weight_no_short_detours,
                        weight_progress_speed,
                        weight_check_distance_to_next,
                        weight_prefer_same_road,
                        weight_no_loops,
                        weight_heading,
                        weight_rules_highway,
                        weight_rules_surface,
                        weight_rules_smoothness,
                    ],
                )
                .generate_routes()
            })
            .filter_map(|nav_route| match nav_route {
                NavigationResult::Stuck => None,
                NavigationResult::Finished(route) => Some(route),
                NavigationResult::Stopped(route) => Some(route),
            })
            .collect::<Vec<_>>();

        let clustering = Clustering::generate(&routes);

        let mut cluster_best: HashMap<i32, RouteWithStats> = HashMap::new();
        let mut noise = Vec::new();
        routes.iter().enumerate().for_each(|(idx, route)| {
            let mut stats = route.calc_stats();
            let approx_route = &clustering.approximated_routes[idx];
            stats.cluster = Some(clustering.labels[idx] as usize);
            stats.approximated_route = approx_route.iter().map(|p| (p[0], p[1])).collect();
            let route_with_stats = RouteWithStats {
                stats,
                route: route.clone(),
            };

            let label = clustering.labels[idx];
            if label != -1 {
                if let Some(current_best) = cluster_best.get(&label) {
                    if current_best.stats.score < route_with_stats.stats.score {
                        cluster_best.insert(label, route_with_stats);
                    }
                } else {
                    cluster_best.insert(label, route_with_stats);
                }
            } else {
                noise.push(route_with_stats);
            }
        });

        trace!(noise_count = noise.len(), "noise");

        let mut best_routes = cluster_best.into_iter().map(|el| el.1).collect::<Vec<_>>();
        noise.sort_by(|a, b| b.stats.score.total_cmp(&a.stats.score));
        best_routes.append(&mut noise[..noise.len().min(3)].to_vec());
        best_routes
    }
}
