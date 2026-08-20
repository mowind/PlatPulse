-- Replace the legacy anonymous-home boolean with the domain-level Site
-- Access Mode and a durable authorization generation.
-- Existing installations migrate without becoming more public than before.

INSERT INTO server_settings (setting_key, setting_value, updated_at)
SELECT 'site_access_mode', CASE WHEN setting_value = '1' THEN 'public' ELSE 'private' END, updated_at
FROM server_settings
WHERE setting_key = 'anonymous_home'
  AND NOT EXISTS (SELECT 1 FROM server_settings WHERE setting_key = 'site_access_mode');

INSERT INTO server_settings (setting_key, setting_value, updated_at)
SELECT 'site_access_mode', 'private', '1970-01-01T00:00:00Z'
WHERE NOT EXISTS (SELECT 1 FROM server_settings WHERE setting_key = 'site_access_mode');

INSERT INTO server_settings (setting_key, setting_value, updated_at)
SELECT 'authorization_generation', '0', '1970-01-01T00:00:00Z'
WHERE NOT EXISTS (SELECT 1 FROM server_settings WHERE setting_key = 'authorization_generation');
