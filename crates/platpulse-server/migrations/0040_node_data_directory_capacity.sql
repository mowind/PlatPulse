-- Store the total capacity of the filesystem containing each Node data directory.
ALTER TABLE current_node_data_directory_observations
    ADD COLUMN capacity_bytes INTEGER CHECK (capacity_bytes IS NULL OR capacity_bytes >= 0);
