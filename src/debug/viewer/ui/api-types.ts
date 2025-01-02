/*
 Generated by typeshare 1.13.2
*/

export interface DebugStreamForkChoiceWeights {
	itinerary_id: string;
	step_num: number;
	end_point_id: string;
	weight_name: string;
	weight_type: string;
	weight_value: number;
}

export interface DebugStreamForkChoices {
	itinerary_id: string;
	step_num: number;
	end_point_id: string;
	line_point_0_lat: number;
	line_point_0_lon: number;
	line_point_1_lat: number;
	line_point_1_lon: number;
	segment_end_point: number;
	discarded: boolean;
}

export interface DebugStreamItineraries {
	itinerary_id: string;
	waypoints_count: number;
	radius: number;
	visit_all: boolean;
	start_lat: number;
	start_lon: number;
	finish_lat: number;
	finish_lon: number;
}

export interface DebugStreamItineraryWaypoints {
	itinerary_id: string;
	idx: number;
	lat: number;
	lon: number;
}

export interface DebugStreamStepResults {
	itinerary_id: string;
	step_num: number;
	result: string;
	chosen_fork_point_id: string;
}

export interface DebugStreamSteps {
	itinerary_id: string;
	step_num: number;
	move_result: string;
}

