pub mod graph;
pub mod line;
pub mod osm;
pub mod point;
pub mod rule;
pub mod way;

#[derive(Debug, PartialEq, Clone)]
pub enum MapDataError {
    MissingPoint {
        point_id: u64,
    },
    MissingRestriction {
        relation_id: u64,
    },
    UnknownRestriction {
        relation_id: u64,
        restriction: String,
    },
    MissingViaNode {
        relation_id: u64,
    },
    MissingViaPoint {
        point_id: u64,
    },
    WayIdNotLinkedWithViaPoint {
        relation_id: u64,
        point_id: u64,
        way_id: u64,
    },
}