-- Operational notifications use the existing actor-attributed event/delivery/outbox path.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
INSERT INTO notification_kinds(kind,self_suppressed,description) VALUES
 ('server_certificate_expiring',false,'Server certificate expires within the reported 90/60/30/7-day threshold'),
 ('disk_headroom_low',false,'Filesystem has less than 15 percent free bytes/inodes or measured backup working space'),
 ('server_certificate_expiring_90',false,'Server certificate expires within 90 days'),
 ('server_certificate_expiring_60',false,'Server certificate expires within 60 days'),
 ('server_certificate_expiring_30',false,'Server certificate expires within 30 days'),
 ('server_certificate_expiring_7',false,'Server certificate expires within 7 days'),
 ('server_certificate_expired',false,'Server certificate has expired'),
 ('server_certificate_renewal_failed',false,'Automatic server certificate renewal failed'),
 ('disk_space_low',false,'Filesystem has less than 15 percent available blocks'),
 ('disk_inodes_low',false,'Filesystem has less than 15 percent available inodes'),
 ('disk_backup_headroom_low',false,'Filesystem has insufficient measured backup working space')
ON CONFLICT (kind) DO NOTHING;
RESET search_path;
RESET ROLE;
