use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
};

use crate::{
    debug_writer::{DebugLoggerFileSink, DebugLoggerVoidSink},
    map_data::graph::MapDataPointRef,
    MAP_DATA_GRAPH,
};

use super::{
    itinerary::Itinerary,
    route::Route,
    walker::{Walker, WalkerMoveResult},
    weights::{WeightCalc, WeightCalcInput},
};

#[derive(Debug, Clone, PartialEq)]
pub enum WeightCalcResult {
    UseWithWeight(u8),
    DoNotUse,
}

#[derive(Debug)]
pub struct DiscardedForkChoices {
    choices: HashMap<MapDataPointRef, HashSet<MapDataPointRef>>,
}
impl DiscardedForkChoices {
    pub fn new() -> Self {
        Self {
            choices: HashMap::new(),
        }
    }

    pub fn add_discarded_choice(
        &mut self,
        point_ref: &MapDataPointRef,
        choice_point_ref: &MapDataPointRef,
    ) -> () {
        let existing_choices = self.choices.get(point_ref);
        if let Some(mut existing_choices) = existing_choices.cloned() {
            existing_choices.insert(choice_point_ref.clone());
            self.choices.insert(point_ref.clone(), existing_choices);
        } else if existing_choices.is_none() {
            let mut ids = HashSet::new();
            ids.insert(choice_point_ref.clone());
            self.choices.insert(point_ref.clone(), ids);
        }
    }

    pub fn get_discarded_choices_for_point(
        &self,
        point_ref: &MapDataPointRef,
    ) -> Option<Vec<MapDataPointRef>> {
        match self.choices.get(point_ref) {
            None => None,
            Some(ids) => Some(ids.clone().into_iter().collect()),
        }
    }
}

#[derive(Clone)]
pub struct ForkWeights {
    weight_list: HashMap<MapDataPointRef, u32>,
}

impl ForkWeights {
    pub fn new() -> Self {
        Self {
            weight_list: HashMap::new(),
        }
    }
    pub fn add_calc_result(
        &mut self,
        choice_point_ref: &MapDataPointRef,
        weights: &Vec<WeightCalcResult>,
    ) -> () {
        if weights
            .iter()
            .all(|weight| *weight != WeightCalcResult::DoNotUse)
        {
            let existing_weight = match self.weight_list.get(choice_point_ref) {
                None => 0u32,
                Some(w) => w.clone(),
            };
            self.weight_list.insert(
                choice_point_ref.clone(),
                existing_weight
                    + weights
                        .into_iter()
                        .map(|r| match r {
                            WeightCalcResult::DoNotUse => 0u32,
                            WeightCalcResult::UseWithWeight(w) => w.clone() as u32,
                        })
                        .sum::<u32>(),
            );
        }
    }

    fn get_choices_sorted_by_weight(&self) -> Vec<(&MapDataPointRef, &u32)> {
        let mut vec = self.weight_list.iter().collect::<Vec<_>>();
        vec.sort_by(|v, v2| v2.1.cmp(v.1));
        vec
    }

    pub fn get_choice_id_by_index_from_heaviest(&self, idx: usize) -> Option<MapDataPointRef> {
        let vec = self.get_choices_sorted_by_weight();
        vec.get(idx).map(|w| w.0).cloned()
    }
}

impl Debug for ForkWeights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.weight_list
                .iter()
                .fold(String::new(), |all, el| format!(
                    "{}\n\t{}:{}",
                    all,
                    el.0.borrow().id,
                    el.1
                ))
        )
    }
}

pub enum NavigationResult {
    Stuck,
    Stopped(Route),
    Finished(Route),
}

pub struct Navigator {
    itinerary: Itinerary,
    walker: Walker,
    weight_calcs: Vec<WeightCalc>,
    discarded_fork_choices: DiscardedForkChoices,
}

impl Navigator {
    pub fn new(itinerary: Itinerary, weight_calcs: Vec<WeightCalc>) -> Self {
        Navigator {
            walker: Walker::new(
                itinerary.get_from().clone(),
                itinerary.get_to().clone(),
                // Box::new(DebugLoggerFileSink::new(
                //     1,
                //     itinerary.get_from().clone(),
                //     itinerary.get_to().clone(),
                //     itinerary.get_waypoints().clone(),
                // )),
                Box::new(DebugLoggerVoidSink::default()),
            ),
            itinerary,
            weight_calcs,
            discarded_fork_choices: DiscardedForkChoices::new(),
        }
    }

    pub fn generate_routes(mut self) -> NavigationResult {
        let mut loop_counter = 0;
        loop {
            loop_counter += 1;
            self.walker.debug_logger.log_step();

            let move_result = self.walker.move_forward_to_next_fork();
            if let Ok(move_result) = &move_result {
                self.walker
                    .debug_logger
                    .log_move(move_result, &self.walker.get_route().clone());
            }

            if move_result == Ok(WalkerMoveResult::Finish) {
                return NavigationResult::Finished(self.walker.get_route().clone());
            }
            if let Ok(WalkerMoveResult::Fork(fork_choices)) = move_result {
                self.walker
                    .debug_logger
                    .log(format!("choices: {:#?}", fork_choices));
                let (fork_choices, last_point) = {
                    let last_point = self.walker.get_last_point();
                    self.walker.debug_logger.log(format!(
                        "discarded choices: {:#?}",
                        self.discarded_fork_choices
                            .get_discarded_choices_for_point(&last_point)
                    ));
                    (
                        fork_choices.exclude_segments_where_points_in(
                            &self
                                .discarded_fork_choices
                                .get_discarded_choices_for_point(&last_point)
                                .map_or(Vec::new(), |d| d),
                        ),
                        last_point,
                    )
                };

                self.itinerary.check_set_next(last_point.clone());

                let fork_weights = fork_choices.clone().into_iter().fold(
                    ForkWeights::new(),
                    |mut fork_weights, fork_route_segment| {
                        let fork_weight_calc_results = self
                            .weight_calcs
                            .iter()
                            .map(|weight_calc| {
                                weight_calc(WeightCalcInput {
                                    route: self.walker.get_route(),
                                    itinerary: &self.itinerary,
                                    current_fork_segment: &fork_route_segment,
                                    all_fork_segments: &fork_choices,
                                    walker_from_fork: Walker::new(
                                        fork_route_segment.get_end_point().clone(),
                                        self.itinerary.get_next().clone(),
                                        Box::new(DebugLoggerVoidSink::default()),
                                    ),
                                    debug_logger: &self.walker.debug_logger,
                                })
                            })
                            .collect::<Vec<_>>();
                        self.walker.debug_logger.log(format!(
                            "weight: {:#?}\n\t{:#?}",
                            fork_route_segment.get_end_point().borrow().id,
                            fork_weight_calc_results.clone(),
                        ));

                        fork_weights.add_calc_result(
                            &fork_route_segment.get_end_point(),
                            &fork_weight_calc_results,
                        );

                        self.walker.debug_logger.log(format!(
                            "calc result: {:#?}\n\t{:#?}",
                            fork_route_segment.get_end_point().borrow().id,
                            &fork_weight_calc_results,
                        ));
                        fork_weights
                    },
                );
                self.walker
                    .debug_logger
                    .log(format!("all weights: {:#?}", &fork_weights));

                let chosen_fork_point = fork_weights.get_choice_id_by_index_from_heaviest(0);

                if let Some(chosen_fork_point) = chosen_fork_point {
                    self.discarded_fork_choices
                        .add_discarded_choice(&last_point, &chosen_fork_point);
                    self.walker.debug_logger.log(format!(
                        "fork action choice: {:#?}",
                        chosen_fork_point.borrow().id
                    ));
                    self.walker.set_fork_choice_point_ref(chosen_fork_point);
                } else {
                    let move_back_segment_list = self.walker.move_backwards_to_prev_fork();
                    let last_segment = self
                        .walker
                        .get_route()
                        .get_segment_last()
                        .map(|s| s.clone());
                    self.walker.debug_logger.log(format!(
                        "fork action go back:\n\tlast:\n\t\t{:#?}\n\tfork:\n\t\t{:#?}",
                        last_segment, move_back_segment_list
                    ));
                    if self.walker.get_route().get_junction_before_last_segment() == None {
                        return NavigationResult::Stuck;
                    }
                }
            } else if move_result == Ok(WalkerMoveResult::DeadEnd) {
                let move_back_segment_list = self.walker.move_backwards_to_prev_fork();
                let last_segment = self
                    .walker
                    .get_route()
                    .get_segment_last()
                    .map(|s| s.clone());
                self.walker.debug_logger.log(format!(
                    "fork action go back:\n\tlast:\n\t\t{:#?}\n\tfork:\n\t\t{:#?}",
                    last_segment, move_back_segment_list
                ));
            }

            if loop_counter >= 1000000 {
                return NavigationResult::Stopped(self.walker.get_route().clone());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        map_data::graph::MapDataGraph,
        router::{
            itinerary::Itinerary,
            navigator::{NavigationResult, WeightCalcResult},
            weights::WeightCalcInput,
        },
        test_utils::{
            graph_from_test_dataset, route_matches_ids, set_graph_static, test_dataset_1,
        },
    };

    use super::Navigator;
    use rusty_fork::rusty_fork_test;

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn navigate_pick_best() {
            fn weight(input: WeightCalcInput) -> WeightCalcResult {
                let prev_point = match input.route.get_segment_last() {
                    Some(segment) => segment.get_end_point(),
                    None => &input.itinerary.get_from().clone(),
                };
                if prev_point.borrow().id == 3
                    && input.current_fork_segment.get_end_point().borrow().id == 6
                {
                    return WeightCalcResult::UseWithWeight(10);
                }
                WeightCalcResult::UseWithWeight(1)
            }
            set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let from = MapDataGraph::get().test_get_point_ref_by_id(&1).unwrap();
            let to = MapDataGraph::get().test_get_point_ref_by_id(&7).unwrap();
            let itinerary = Itinerary::new(from, to, Vec::new(), 0.);
            let mut navigator = Navigator::new(itinerary.clone(), vec![weight]);
            let route = match navigator.generate_routes() {
                crate::router::navigator::NavigationResult::Finished(r) => r,
                _ => {
                    assert!(false);
                    return ();
                }
            };

            assert!(route_matches_ids(route.clone(), vec![2, 3, 6, 7]));

            fn weight2(input: WeightCalcInput) -> WeightCalcResult {
                let prev_point = match input.route.get_segment_last() {
                    Some(segment) => segment.get_end_point(),
                    None => &input.itinerary.get_to().clone(),
                };

                if prev_point.borrow().id == 3
                    && input.current_fork_segment.get_end_point().borrow().id == 4
                {
                    return WeightCalcResult::UseWithWeight(10);
                }
                WeightCalcResult::UseWithWeight(1)
            }
            let mut navigator = Navigator::new(itinerary, vec![weight2]);
            let route = match navigator.generate_routes() {
                crate::router::navigator::NavigationResult::Finished(r) => r,
                _ => {
                    assert!(false);
                    return ();
                }
            };

            assert!(route_matches_ids(route.clone(), vec![2, 3, 4, 8, 6, 7]));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn navigate_dead_end_pick_next_best() {
            fn weight(input: WeightCalcInput) -> WeightCalcResult {
                let prev_point = match input.route.get_segment_last() {
                    Some(segment) => segment.get_end_point(),
                    None => &input.itinerary.get_to().clone(),
                };

                if prev_point.borrow().id == 3 {
                    if input.current_fork_segment.get_end_point().borrow().id == 5 {
                        return WeightCalcResult::UseWithWeight(10);
                    }
                    if input.current_fork_segment.get_end_point().borrow().id == 6 {
                        return WeightCalcResult::UseWithWeight(5);
                    }
                }
                if prev_point.borrow().id == 6
                    && input.current_fork_segment.get_end_point().borrow().id == 7
                {
                    return WeightCalcResult::UseWithWeight(10);
                }
                WeightCalcResult::UseWithWeight(1)
            }
            set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let from = MapDataGraph::get().test_get_point_ref_by_id(&1).unwrap();
            let to = MapDataGraph::get().test_get_point_ref_by_id(&7).unwrap();
            let itinerary = Itinerary::new(from, to, Vec::new(), 0.);
            let mut navigator = Navigator::new(itinerary, vec![weight]);
            let route = match navigator.generate_routes() {
                crate::router::navigator::NavigationResult::Finished(r) => r,
                _ => {
                    assert!(false);
                    return ();
                }
            };

            assert!(route_matches_ids(route.clone(), vec![2, 3, 6, 7]));
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn navigate_all_stuck_return_no_routes() {
            fn weight(_input: WeightCalcInput) -> WeightCalcResult {
                WeightCalcResult::UseWithWeight(1)
            }
            set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let from = MapDataGraph::get().test_get_point_ref_by_id(&1).unwrap();
            let to = MapDataGraph::get().test_get_point_ref_by_id(&11).unwrap();
            let itinerary = Itinerary::new(from, to, Vec::new(), 0.);
            let mut navigator = Navigator::new( itinerary, vec![weight]);

            if let NavigationResult::Finished(_) = navigator.generate_routes() {
                assert!(false);
            }
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn navigate_no_routes_with_do_not_use_weight() {
            fn weight(input: WeightCalcInput) -> WeightCalcResult {
                if input.current_fork_segment.get_end_point().borrow().id == 7 {
                    return WeightCalcResult::DoNotUse;
                }
                WeightCalcResult::UseWithWeight(1)
            }
            set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let from = MapDataGraph::get().test_get_point_ref_by_id(&1).unwrap();
            let to = MapDataGraph::get().test_get_point_ref_by_id(&7).unwrap();
            let itinerary = Itinerary::new(from, to, Vec::new(), 0.);
            let mut navigator = Navigator::new(itinerary, vec![weight]);
            if let NavigationResult::Finished(_) = navigator.generate_routes() {
                assert!(false);
            }
        }
    }

    rusty_fork_test! {
        #![rusty_fork(timeout_ms = 2000)]
        #[test]
        fn navigate_on_weight_sum() {
            fn weight1(input: WeightCalcInput) -> WeightCalcResult {
                let prev_point = match input.route.get_segment_last() {
                    Some(segment) => segment.get_end_point(),
                    None => &input.itinerary.get_to().clone(),
                };
                if prev_point.borrow().id == 3
                    && input.current_fork_segment.get_end_point().borrow().id == 6
                {
                    return WeightCalcResult::UseWithWeight(10);
                }
                WeightCalcResult::UseWithWeight(6)
            }
            fn weight2(input: WeightCalcInput) -> WeightCalcResult {
                let prev_point = match input.route.get_segment_last() {
                    Some(segment) => segment.get_end_point(),
                    None => &input.itinerary.get_to().clone(),
                };

                if prev_point.borrow().id == 3
                    && input.current_fork_segment.get_end_point().borrow().id == 6
                {
                    return WeightCalcResult::UseWithWeight(1);
                }
                WeightCalcResult::UseWithWeight(6)
            }
            set_graph_static(graph_from_test_dataset(test_dataset_1()));
            let from = MapDataGraph::get().test_get_point_ref_by_id(&1).unwrap();
            let to = MapDataGraph::get().test_get_point_ref_by_id(&7).unwrap();
            let itinerary = Itinerary::new(from, to, Vec::new(), 0.);
            let mut navigator = Navigator::new(itinerary, vec![weight1, weight2]);
            let route = match navigator.generate_routes() {
                crate::router::navigator::NavigationResult::Finished(r) => r,
                _ => {
                    assert!(false);
                    return ();
                }
            };
            assert!(route_matches_ids(route.clone(), vec![2, 3, 4, 8, 6, 7]));
        }
    }
}
