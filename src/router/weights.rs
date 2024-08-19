use std::rc::Rc;

use geo::{HaversineBearing, Point};

use super::{
    itinerary::Itinerary,
    navigator::WeightCalcResult,
    route::{segment::Segment, segment_list::SegmentList, Route},
    walker::{Walker, WalkerMoveResult},
};

pub struct WeightCalcInput<'a> {
    pub current_fork_segment: &'a Segment,
    pub route: &'a Route,
    pub all_fork_segments: &'a SegmentList,
    pub itinerary: &'a Itinerary,
    pub walker_from_fork: Walker<'a>,
}

pub type WeightCalc = fn(input: WeightCalcInput) -> WeightCalcResult;

pub fn weight_heading(input: WeightCalcInput) -> WeightCalcResult {
    let mut walker = input.walker_from_fork;
    let next_fork = match walker.move_forward_to_next_fork() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("weight calc error {:#?}", e);
            return WeightCalcResult::DoNotUse;
        }
    };
    let _ = match next_fork {
        WalkerMoveResult::DeadEnd => return WeightCalcResult::DoNotUse,
        WalkerMoveResult::Finish => return WeightCalcResult::UseWithWeight(255),
        WalkerMoveResult::Fork(f) => f,
    };
    let fork_segment = match walker.get_route().get_segment_last() {
        Some(last_segment) => last_segment,
        None => input.current_fork_segment,
    };
    let fork_point_geo = Point::new(
        fork_segment.get_end_point().borrow().lon,
        fork_segment.get_end_point().borrow().lat,
    );
    let next_point_geo = Point::new(
        input.itinerary.get_next().borrow().lon,
        input.itinerary.get_next().borrow().lat,
    );
    let next_bearing = fork_point_geo.haversine_bearing(next_point_geo);
    let fork_line_one_geo = Point::new(
        fork_segment.get_line().borrow().points.0.borrow().lon,
        fork_segment.get_line().borrow().points.0.borrow().lat,
    );
    let fork_line_two_geo = Point::new(
        fork_segment.get_line().borrow().points.1.borrow().lon,
        fork_segment.get_line().borrow().points.1.borrow().lat,
    );
    let fork_bearing = if &fork_segment.get_line().borrow().points.1 == fork_segment.get_end_point()
    {
        fork_line_one_geo.haversine_bearing(fork_line_two_geo)
    } else {
        fork_line_two_geo.haversine_bearing(fork_line_one_geo)
    };

    let degree_offset_from_next = (fork_bearing - next_bearing).abs();

    let ratio: f64 = 255.0 / 180.0;

    WeightCalcResult::UseWithWeight(255 - (degree_offset_from_next * ratio).round() as u8)
}

pub fn weight_prefer_same_road(input: WeightCalcInput) -> WeightCalcResult {
    let current_ref = input
        .route
        .get_segment_last()
        .map_or(None, |s| s.get_line().borrow().tags_ref.clone());
    let current_name = input
        .route
        .get_segment_last()
        .map_or(None, |s| s.get_line().borrow().tags_name.clone());
    let fork_ref = input
        .current_fork_segment
        .get_line()
        .borrow()
        .tags_ref
        .clone();
    let fork_name = input
        .current_fork_segment
        .get_line()
        .borrow()
        .tags_name
        .clone();

    if current_ref == fork_ref || current_name == fork_name {
        return WeightCalcResult::UseWithWeight(80);
    }

    WeightCalcResult::UseWithWeight(0)
}

pub fn weight_no_loops(input: WeightCalcInput) -> WeightCalcResult {
    if input.route.has_looped() {
        return WeightCalcResult::DoNotUse;
    }

    WeightCalcResult::UseWithWeight(0)
}

pub fn weight_check_distance_to_next(input: WeightCalcInput) -> WeightCalcResult {
    let check_steps_back = 100;

    let distance_to_end_current = match input.route.get_segment_last() {
        None => return WeightCalcResult::UseWithWeight(0),
        Some(segment) => segment
            .get_end_point()
            .borrow()
            .distance_between(&input.itinerary.get_next()),
    };

    let distance_to_end_steps_back = match input.route.get_steps_from_end(check_steps_back) {
        None => return WeightCalcResult::UseWithWeight(0),
        Some(segment) => segment
            .get_end_point()
            .borrow()
            .distance_between(&input.itinerary.get_next()),
    };

    if distance_to_end_current > distance_to_end_steps_back {
        return WeightCalcResult::DoNotUse;
    }
    WeightCalcResult::UseWithWeight(0)
}

pub fn weight_progress_speed(input: WeightCalcInput) -> WeightCalcResult {
    let check_steps_back = 100;

    let current_point = match input.route.get_segment_last() {
        None => return WeightCalcResult::UseWithWeight(0),
        Some(segment) => segment.get_end_point(),
    };

    let total_distance = input
        .itinerary
        .get_from()
        .borrow()
        .distance_between(&input.itinerary.get_next());
    let point_steps_back = match input.route.get_steps_from_end(check_steps_back) {
        None => return WeightCalcResult::UseWithWeight(0),
        Some(segment) => Rc::clone(segment.get_end_point()),
    };

    let average_distance_per_segment = total_distance / (input.route.get_segment_count() as f64);

    let distance_last_points = point_steps_back.borrow().distance_between(&current_point);
    let average_distance_last_points = distance_last_points / (check_steps_back as f64);

    if average_distance_last_points < average_distance_per_segment * 0.3 {
        // return WeightCalcResult::DoNotUse;
        return WeightCalcResult::UseWithWeight(0);
    }

    WeightCalcResult::UseWithWeight(0)
}

#[cfg(test)]
mod test {

    use crate::{
        debug_writer::DebugLoggerVoidSink,
        map_data_graph::MapDataPointRef,
        osm_data_reader::OsmDataReader,
        router::{
            itinerary::Itinerary,
            navigator::WeightCalcResult,
            route::{segment::Segment, segment_list::SegmentList},
            walker::Walker,
        },
    };

    use super::{weight_heading, WeightCalcInput};

    fn get_route_segment(
        end_point: MapDataPointRef,
        opposite_point_for_line: MapDataPointRef,
    ) -> Segment {
        let end_point_borrowed = end_point.borrow();
        let line = end_point_borrowed
            .lines
            .iter()
            .find(|line| {
                let line = line.borrow();
                (line.points.0 == end_point && line.points.1 == opposite_point_for_line)
                    || (line.points.1 == end_point && line.points.0 == opposite_point_for_line)
            })
            .expect("line to be found");

        Segment::new(line.clone(), end_point.clone())
    }

    #[test]
    fn weight_heading_test() {
        let data_reader = OsmDataReader::new_file(String::from("test-data/sigulda-100.json"));
        let map_data = data_reader.read_data().expect("to load test file");
        let from = map_data
            .get_point_by_id(&885564366)
            .expect("to find start point");
        let to = map_data
            .get_point_by_id(&33416714)
            .expect("to find end point");
        let disabled_debug_writer = Box::new(DebugLoggerVoidSink::default());
        let walker = Walker::new(
            &map_data,
            from.clone(),
            to.clone(),
            disabled_debug_writer.clone(),
        );

        let fork_point = map_data
            .get_point_by_id(&81272994)
            .expect("to find fork point");

        let segment = get_route_segment(fork_point, from.clone());

        let itinerary = Itinerary::new(from.clone(), to.clone(), Vec::new(), 0.);

        let fork_weight = weight_heading(WeightCalcInput {
            route: walker.get_route(),
            itinerary: &itinerary,
            all_fork_segments: &SegmentList::from(vec![]),
            current_fork_segment: &segment,
            walker_from_fork: Walker::new(
                &map_data,
                from.clone(),
                to.clone(),
                disabled_debug_writer.clone(),
            ),
        });
        eprintln!("{:#?}", fork_weight);
        assert_eq!(fork_weight, WeightCalcResult::UseWithWeight(177));

        let fork_point = map_data
            .get_point_by_id(&9212889586)
            .expect("to find fork point");

        let segment = get_route_segment(fork_point, from.clone());

        let fork_weight = weight_heading(WeightCalcInput {
            route: walker.get_route(),
            itinerary: &itinerary,
            all_fork_segments: &SegmentList::from(vec![]),
            current_fork_segment: &segment,
            walker_from_fork: Walker::new(
                &map_data,
                from.clone(),
                to.clone(),
                disabled_debug_writer.clone(),
            ),
        });
        eprintln!("{:#?}", fork_weight);
        assert_eq!(fork_weight, WeightCalcResult::UseWithWeight(64));
    }
}