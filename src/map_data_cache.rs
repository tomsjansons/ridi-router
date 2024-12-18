use std::{io, path::PathBuf, time::Instant};

use rayon::iter::{
    IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelIterator,
};
use tracing::info;

use crate::map_data::graph::MapDataGraphPacked;

fn read_cache_file(file_folder: &PathBuf, file_name: &str) -> Result<Vec<u8>, MapDataCacheError> {
    let mut file = file_folder.clone();
    file.push(format!("{file_name}.cache"));
    let file_contents =
        std::fs::read(file).map_err(|error| MapDataCacheError::FileError { error })?;

    Ok(file_contents)
}
fn write_cache_file(
    file_folder: &PathBuf,
    file_name: &str,
    file_contents: &Vec<u8>,
) -> Result<(), MapDataCacheError> {
    let mut file = file_folder.clone();
    file.push(format!("{file_name}.cache"));
    std::fs::write(file, file_contents).map_err(|error| MapDataCacheError::FileError { error })?;

    Ok(())
}

#[derive(Debug)]
pub enum MapDataCacheError {
    FileError { error: io::Error },
    MissingValue,
    UnexpectedValue,
}
pub struct MapDataCache {
    cache_dir: Option<PathBuf>,
    write_to_cache: bool,
}

impl MapDataCache {
    pub fn init(cache_dir: Option<PathBuf>) -> Self {
        Self {
            write_to_cache: cache_dir.is_some(),
            cache_dir,
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn read_cache(&mut self) -> Result<Option<MapDataGraphPacked>, MapDataCacheError> {
        let cache_dir = match &self.cache_dir {
            None => return Ok(None),
            Some(cd) => cd,
        };

        let read_start = Instant::now();

        if !std::fs::exists(&cache_dir).map_err(|error| MapDataCacheError::FileError { error })? {
            return Ok(None);
        }

        self.write_to_cache = false;

        let mut points: Option<Result<Vec<u8>, MapDataCacheError>> = None;
        let mut point_grid: Option<Result<Vec<u8>, MapDataCacheError>> = None;
        let mut lines: Option<Result<Vec<u8>, MapDataCacheError>> = None;
        let mut tags: Option<Result<Vec<u8>, MapDataCacheError>> = None;
        rayon::scope(|scope| {
            scope.spawn(|_| {
                points = Some(read_cache_file(&cache_dir, "points"));
            });
            scope.spawn(|_| {
                point_grid = Some(read_cache_file(&cache_dir, "point_grid"));
            });
            scope.spawn(|_| {
                lines = Some(read_cache_file(&cache_dir, "lines"));
            });
            scope.spawn(|_| {
                tags = Some(read_cache_file(&cache_dir, "tags"));
            });
        });

        let packed_data = MapDataGraphPacked {
            points: points.ok_or(MapDataCacheError::MissingValue)??,
            point_grid: point_grid.ok_or(MapDataCacheError::MissingValue)??,
            lines: lines.ok_or(MapDataCacheError::MissingValue)??,
            tags: tags.ok_or(MapDataCacheError::MissingValue)??,
        };

        let read_duration = read_start.elapsed();
        info!("cache read took {} seconds", read_duration.as_secs());

        Ok(Some(packed_data))
    }

    #[tracing::instrument(skip(self, packed_data))]
    pub fn write_cache(&self, packed_data: MapDataGraphPacked) -> Result<(), MapDataCacheError> {
        if !self.write_to_cache {
            return Ok(());
        }

        let write_start = Instant::now();

        if let Some(cache_dir) = &self.cache_dir {
            if std::fs::exists(&cache_dir)
                .map_err(|error| MapDataCacheError::FileError { error })?
            {
                std::fs::remove_dir_all(&cache_dir)
                    .map_err(|error| MapDataCacheError::FileError { error })?;
            }
            std::fs::create_dir_all(&cache_dir)
                .map_err(|error| MapDataCacheError::FileError { error })?;

            let tasks = [0u8; 4];
            tasks
                .par_iter()
                .enumerate()
                .map(|(i, _)| match i {
                    0 => write_cache_file(&cache_dir, "points", &packed_data.points),
                    1 => write_cache_file(&cache_dir, "point_grid", &packed_data.point_grid),
                    2 => write_cache_file(&cache_dir, "lines", &packed_data.lines),
                    3 => write_cache_file(&cache_dir, "tags", &packed_data.tags),
                    _ => Err(MapDataCacheError::UnexpectedValue),
                })
                .collect::<Result<Vec<_>, MapDataCacheError>>()?;
        }
        let write_end = write_start.elapsed();
        info!("cache write {}s", write_end.as_secs());
        Ok(())
    }
}
