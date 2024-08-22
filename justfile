gps-query-range := '100000' # 100km
gps-query-from := '56.951861,24.113821' # riga
gps-query-to := '57.313103,25.281460' # cesis
map-data-file-name := "map-data-riga-cesis.json"

# gps-query-range := '100' # 100m
# gps-query-from := '57.155453,24.853327' # sigulda
# gps-query-to := '57.155453,24.853327' # sigulda
# map-data-file-name := "test-data-sig-100.json"

overpass-query := '"[out:json];
                    way
                      [highway]
                      [access!=private]
                      [highway!=footway]
                      [motor_vehicle!=private]
                      [motor_vehicle!=no]
                      [!service]
                      [highway!=cycleway]
                      [highway!=steps]
                      [highway!=pedestrian]
                      [access!=no]
                      [highway!=path]
                      [highway!=service]
                      (around:' + gps-query-range + ',' + gps-query-from + ',' + gps-query-to + ')->.roads;
                    relation
                      [type=restriction]
                      (around:' + gps-query-range + ',' + gps-query-from + ',' + gps-query-to + ')->.rules;
                    (
                      .roads;>>;
                      .rules;>>;
                    );
                    out;"'

data-fetch:
  curl --data {{overpass-query}} "https://overpass-api.de/api/interpreter" > map-data/{{map-data-file-name}}

gps-test-from-lat := '57.154260' # sigulda
gps-test-from-lon := '24.853496' # sigulda
gps-test-to-lat := '56.856551'		# doles sala
gps-test-to-lon := '24.253038'		# doles sala
# gps-test-to-lat := '57.111708'		# garciems
# gps-test-to-lon := '24.192656'		# garciems

run-and-load-stdin := 'cat map-data' / map-data-file-name + ' | cargo run -- --from_lat ' + gps-test-from-lat + ' --from_lon ' + gps-test-from-lon + ' --to_lat ' + gps-test-to-lat + ' --to_lon ' + gps-test-to-lon

run-stdin:
  {{run-and-load-stdin}}

run-show-stdin:
  {{run-and-load-stdin}} > map-data/output.gpx
  gpxsee map-data/output.gpx &
  
run-and-load-file := 'cargo run -- --data_file map-data' / map-data-file-name + ' --from_lat ' + gps-test-from-lat + ' --from_lon ' + gps-test-from-lon + ' --to_lat ' + gps-test-to-lat + ' --to_lon ' + gps-test-to-lon

run-file:
  {{run-and-load-file}}

run-show-file:
  {{run-and-load-file}} > map-data/output.gpx
  gpxsee map-data/output.gpx &
