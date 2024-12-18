use geo::Point;
use gpx::{write, Gpx, GpxVersion, Route as GpxRoute, Waypoint};
use std::{collections::HashMap, fs::File, io::Error, path::PathBuf};

use crate::{ipc_handler::RouteMessage, router::route::RouteStatElement};

#[derive(Debug)]
pub enum GpxWriterError {
    FileCreateError { error: Error },
}

pub struct GpxWriter {
    routes: Vec<RouteMessage>,
    file_name: PathBuf,
}

fn sort_by_longest(map: HashMap<String, RouteStatElement>) -> Vec<(String, RouteStatElement)> {
    let mut vec = Vec::from_iter(map.into_iter());
    vec.sort_by(|a, b| b.1.len_m.total_cmp(&a.1.len_m));
    vec
}

impl GpxWriter {
    pub fn new(routes: Vec<RouteMessage>, file_name: PathBuf) -> Self {
        Self { routes, file_name }
    }
    pub fn write_gpx(self) -> Result<(), GpxWriterError> {
        let mut gpx = Gpx::default();
        gpx.version = GpxVersion::Gpx11;

        let mut csv_contents =
            String::from("id,len,junctions,mean_point_lat,mean_point_lon,dir_change_ratio\n");
        for (idx, route) in self.routes.into_iter().enumerate() {
            csv_contents.push_str(&format!(
                "r_{},{},{},{},{},{}\n",
                idx,
                route.stats.len_m / 1000.,
                route.stats.junction_count,
                route.stats.mean_point.lat,
                route.stats.mean_point.lon,
                route.stats.direction_change_ratio
            ));
            let mut gpx_route = GpxRoute::new();
            gpx_route.name = Some(format!("r_{idx}"));

            let mut description = String::new();
            description.push_str(&format!("Length: {:.2}km\n", route.stats.len_m / 1000.));
            description.push_str(&format!(
                "Number of junctions: {}\n",
                route.stats.junction_count
            ));
            description.push_str(&format!(
                "Mean point: {:.5},{:.5}\n",
                route.stats.mean_point.lat, route.stats.mean_point.lon
            ));
            description.push_str(&format!(
                "Direction change degrees per km: {:.2}\n",
                route.stats.direction_change_ratio
            ));
            description.push_str(&format!("Road types:\n"));
            for (road_type, stat) in sort_by_longest(route.stats.highway).iter() {
                description.push_str(&format!(
                    " - {road_type}: {:.2}km, {:.2}%\n",
                    stat.len_m / 1000.,
                    stat.percentage,
                ));
            }
            description.push_str(&format!("Road surface:\n"));
            for (surface_type, stat) in sort_by_longest(route.stats.surface).iter() {
                description.push_str(&format!(
                    " - {surface_type}: {:.2}km, {:.2}%\n",
                    stat.len_m / 1000.,
                    stat.percentage,
                ));
            }
            description.push_str(&format!("Road smoothness:\n"));
            for (smoothness_type, stat) in sort_by_longest(route.stats.smoothness).iter() {
                description.push_str(&format!(
                    " - {smoothness_type}: {:.2}km, {:.2}%\n",
                    stat.len_m / 1000.,
                    stat.percentage,
                ));
            }

            gpx_route.description = Some(description);

            for coord in &route.coords {
                let waypoint = Waypoint::new(Point::new(coord.lon.into(), coord.lat.into()));
                gpx_route.points.push(waypoint);
            }

            gpx.routes.push(gpx_route);
        }

        let mut csv_filename = PathBuf::from(&self.file_name);
        csv_filename.set_extension("csv");
        std::fs::write(csv_filename, csv_contents).unwrap();
        let file = File::create(self.file_name)
            .or_else(|error| Err(GpxWriterError::FileCreateError { error }))?;

        write(&gpx, file).unwrap();

        Ok(())
    }
}
