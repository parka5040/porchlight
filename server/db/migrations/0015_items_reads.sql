SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- R35b: canonicalize existing storage before tightening the validator. This static
-- link map mirrors core time_links.rs; new writes canonicalize in core validation.
CREATE FUNCTION pl_storage_tzid(z text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT COALESCE((SELECT target FROM (VALUES
   ('Africa/Accra', 'Africa/Abidjan'),
   ('Africa/Addis_Ababa', 'Africa/Nairobi'),
   ('Africa/Asmara', 'Africa/Nairobi'),
   ('Africa/Asmera', 'Africa/Nairobi'),
   ('Africa/Bamako', 'Africa/Abidjan'),
   ('Africa/Bangui', 'Africa/Lagos'),
   ('Africa/Banjul', 'Africa/Abidjan'),
   ('Africa/Blantyre', 'Africa/Maputo'),
   ('Africa/Brazzaville', 'Africa/Lagos'),
   ('Africa/Bujumbura', 'Africa/Maputo'),
   ('Africa/Conakry', 'Africa/Abidjan'),
   ('Africa/Dakar', 'Africa/Abidjan'),
   ('Africa/Dar_es_Salaam', 'Africa/Nairobi'),
   ('Africa/Djibouti', 'Africa/Nairobi'),
   ('Africa/Douala', 'Africa/Lagos'),
   ('Africa/Freetown', 'Africa/Abidjan'),
   ('Africa/Gaborone', 'Africa/Maputo'),
   ('Africa/Harare', 'Africa/Maputo'),
   ('Africa/Kampala', 'Africa/Nairobi'),
   ('Africa/Kigali', 'Africa/Maputo'),
   ('Africa/Kinshasa', 'Africa/Lagos'),
   ('Africa/Libreville', 'Africa/Lagos'),
   ('Africa/Lome', 'Africa/Abidjan'),
   ('Africa/Luanda', 'Africa/Lagos'),
   ('Africa/Lubumbashi', 'Africa/Maputo'),
   ('Africa/Lusaka', 'Africa/Maputo'),
   ('Africa/Malabo', 'Africa/Lagos'),
   ('Africa/Maseru', 'Africa/Johannesburg'),
   ('Africa/Mbabane', 'Africa/Johannesburg'),
   ('Africa/Mogadishu', 'Africa/Nairobi'),
   ('Africa/Niamey', 'Africa/Lagos'),
   ('Africa/Nouakchott', 'Africa/Abidjan'),
   ('Africa/Ouagadougou', 'Africa/Abidjan'),
   ('Africa/Porto-Novo', 'Africa/Lagos'),
   ('Africa/Timbuktu', 'Africa/Abidjan'),
   ('America/Anguilla', 'America/Puerto_Rico'),
   ('America/Antigua', 'America/Puerto_Rico'),
   ('America/Argentina/ComodRivadavia', 'America/Argentina/Catamarca'),
   ('America/Aruba', 'America/Puerto_Rico'),
   ('America/Atikokan', 'America/Panama'),
   ('America/Atka', 'America/Adak'),
   ('America/Blanc-Sablon', 'America/Puerto_Rico'),
   ('America/Buenos_Aires', 'America/Argentina/Buenos_Aires'),
   ('America/Catamarca', 'America/Argentina/Catamarca'),
   ('America/Cayman', 'America/Panama'),
   ('America/Coral_Harbour', 'America/Panama'),
   ('America/Cordoba', 'America/Argentina/Cordoba'),
   ('America/Creston', 'America/Phoenix'),
   ('America/Curacao', 'America/Puerto_Rico'),
   ('America/Dominica', 'America/Puerto_Rico'),
   ('America/Ensenada', 'America/Tijuana'),
   ('America/Fort_Wayne', 'America/Indiana/Indianapolis'),
   ('America/Godthab', 'America/Nuuk'),
   ('America/Grenada', 'America/Puerto_Rico'),
   ('America/Guadeloupe', 'America/Puerto_Rico'),
   ('America/Indianapolis', 'America/Indiana/Indianapolis'),
   ('America/Jujuy', 'America/Argentina/Jujuy'),
   ('America/Knox_IN', 'America/Indiana/Knox'),
   ('America/Kralendijk', 'America/Puerto_Rico'),
   ('America/Louisville', 'America/Kentucky/Louisville'),
   ('America/Lower_Princes', 'America/Puerto_Rico'),
   ('America/Marigot', 'America/Puerto_Rico'),
   ('America/Mendoza', 'America/Argentina/Mendoza'),
   ('America/Montreal', 'America/Toronto'),
   ('America/Montserrat', 'America/Puerto_Rico'),
   ('America/Nassau', 'America/Toronto'),
   ('America/Nipigon', 'America/Toronto'),
   ('America/Pangnirtung', 'America/Iqaluit'),
   ('America/Port_of_Spain', 'America/Puerto_Rico'),
   ('America/Porto_Acre', 'America/Rio_Branco'),
   ('America/Rainy_River', 'America/Winnipeg'),
   ('America/Rosario', 'America/Argentina/Cordoba'),
   ('America/Santa_Isabel', 'America/Tijuana'),
   ('America/Shiprock', 'America/Denver'),
   ('America/St_Barthelemy', 'America/Puerto_Rico'),
   ('America/St_Kitts', 'America/Puerto_Rico'),
   ('America/St_Lucia', 'America/Puerto_Rico'),
   ('America/St_Thomas', 'America/Puerto_Rico'),
   ('America/St_Vincent', 'America/Puerto_Rico'),
   ('America/Thunder_Bay', 'America/Toronto'),
   ('America/Tortola', 'America/Puerto_Rico'),
   ('America/Virgin', 'America/Puerto_Rico'),
   ('America/Yellowknife', 'America/Edmonton'),
   ('Antarctica/DumontDUrville', 'Pacific/Port_Moresby'),
   ('Antarctica/McMurdo', 'Pacific/Auckland'),
   ('Antarctica/South_Pole', 'Pacific/Auckland'),
   ('Antarctica/Syowa', 'Asia/Riyadh'),
   ('Arctic/Longyearbyen', 'Europe/Berlin'),
   ('Asia/Aden', 'Asia/Riyadh'),
   ('Asia/Ashkhabad', 'Asia/Ashgabat'),
   ('Asia/Bahrain', 'Asia/Qatar'),
   ('Asia/Brunei', 'Asia/Kuching'),
   ('Asia/Calcutta', 'Asia/Kolkata'),
   ('Asia/Choibalsan', 'Asia/Ulaanbaatar'),
   ('Asia/Chongqing', 'Asia/Shanghai'),
   ('Asia/Chungking', 'Asia/Shanghai'),
   ('Asia/Dacca', 'Asia/Dhaka'),
   ('Asia/Harbin', 'Asia/Shanghai'),
   ('Asia/Istanbul', 'Europe/Istanbul'),
   ('Asia/Kashgar', 'Asia/Urumqi'),
   ('Asia/Katmandu', 'Asia/Kathmandu'),
   ('Asia/Kuala_Lumpur', 'Asia/Singapore'),
   ('Asia/Kuwait', 'Asia/Riyadh'),
   ('Asia/Macao', 'Asia/Macau'),
   ('Asia/Muscat', 'Asia/Dubai'),
   ('Asia/Phnom_Penh', 'Asia/Bangkok'),
   ('Asia/Rangoon', 'Asia/Yangon'),
   ('Asia/Saigon', 'Asia/Ho_Chi_Minh'),
   ('Asia/Tel_Aviv', 'Asia/Jerusalem'),
   ('Asia/Thimbu', 'Asia/Thimphu'),
   ('Asia/Ujung_Pandang', 'Asia/Makassar'),
   ('Asia/Ulan_Bator', 'Asia/Ulaanbaatar'),
   ('Asia/Vientiane', 'Asia/Bangkok'),
   ('Atlantic/Faeroe', 'Atlantic/Faroe'),
   ('Atlantic/Jan_Mayen', 'Europe/Berlin'),
   ('Atlantic/Reykjavik', 'Africa/Abidjan'),
   ('Atlantic/St_Helena', 'Africa/Abidjan'),
   ('Australia/ACT', 'Australia/Sydney'),
   ('Australia/Canberra', 'Australia/Sydney'),
   ('Australia/Currie', 'Australia/Hobart'),
   ('Australia/LHI', 'Australia/Lord_Howe'),
   ('Australia/NSW', 'Australia/Sydney'),
   ('Australia/North', 'Australia/Darwin'),
   ('Australia/Queensland', 'Australia/Brisbane'),
   ('Australia/South', 'Australia/Adelaide'),
   ('Australia/Tasmania', 'Australia/Hobart'),
   ('Australia/Victoria', 'Australia/Melbourne'),
   ('Australia/West', 'Australia/Perth'),
   ('Australia/Yancowinna', 'Australia/Broken_Hill'),
   ('Brazil/Acre', 'America/Rio_Branco'),
   ('Brazil/DeNoronha', 'America/Noronha'),
   ('Brazil/East', 'America/Sao_Paulo'),
   ('Brazil/West', 'America/Manaus'),
   ('CET', 'Europe/Brussels'),
   ('CST6CDT', 'America/Chicago'),
   ('Canada/Atlantic', 'America/Halifax'),
   ('Canada/Central', 'America/Winnipeg'),
   ('Canada/Eastern', 'America/Toronto'),
   ('Canada/Mountain', 'America/Edmonton'),
   ('Canada/Newfoundland', 'America/St_Johns'),
   ('Canada/Pacific', 'America/Vancouver'),
   ('Canada/Saskatchewan', 'America/Regina'),
   ('Canada/Yukon', 'America/Whitehorse'),
   ('Chile/Continental', 'America/Santiago'),
   ('Chile/EasterIsland', 'Pacific/Easter'),
   ('Cuba', 'America/Havana'),
   ('EET', 'Europe/Athens'),
   ('EST', 'America/Panama'),
   ('EST5EDT', 'America/New_York'),
   ('Egypt', 'Africa/Cairo'),
   ('Eire', 'Europe/Dublin'),
   ('Etc/GMT', 'UTC'),
   ('Etc/GMT+0', 'UTC'),
   ('Etc/GMT-0', 'UTC'),
   ('Etc/GMT0', 'UTC'),
   ('Etc/Greenwich', 'UTC'),
   ('Etc/UCT', 'UTC'),
   ('Etc/UTC', 'UTC'),
   ('Etc/Universal', 'UTC'),
   ('Etc/Zulu', 'UTC'),
   ('Europe/Amsterdam', 'Europe/Brussels'),
   ('Europe/Belfast', 'Europe/London'),
   ('Europe/Bratislava', 'Europe/Prague'),
   ('Europe/Busingen', 'Europe/Zurich'),
   ('Europe/Copenhagen', 'Europe/Berlin'),
   ('Europe/Guernsey', 'Europe/London'),
   ('Europe/Isle_of_Man', 'Europe/London'),
   ('Europe/Jersey', 'Europe/London'),
   ('Europe/Kiev', 'Europe/Kyiv'),
   ('Europe/Ljubljana', 'Europe/Belgrade'),
   ('Europe/Luxembourg', 'Europe/Brussels'),
   ('Europe/Mariehamn', 'Europe/Helsinki'),
   ('Europe/Monaco', 'Europe/Paris'),
   ('Europe/Nicosia', 'Asia/Nicosia'),
   ('Europe/Oslo', 'Europe/Berlin'),
   ('Europe/Podgorica', 'Europe/Belgrade'),
   ('Europe/San_Marino', 'Europe/Rome'),
   ('Europe/Sarajevo', 'Europe/Belgrade'),
   ('Europe/Skopje', 'Europe/Belgrade'),
   ('Europe/Stockholm', 'Europe/Berlin'),
   ('Europe/Tiraspol', 'Europe/Chisinau'),
   ('Europe/Uzhgorod', 'Europe/Kyiv'),
   ('Europe/Vaduz', 'Europe/Zurich'),
   ('Europe/Vatican', 'Europe/Rome'),
   ('Europe/Zagreb', 'Europe/Belgrade'),
   ('Europe/Zaporozhye', 'Europe/Kyiv'),
   ('GB', 'Europe/London'),
   ('GB-Eire', 'Europe/London'),
   ('GMT', 'UTC'),
   ('GMT+0', 'UTC'),
   ('GMT-0', 'UTC'),
   ('GMT0', 'UTC'),
   ('Greenwich', 'UTC'),
   ('HST', 'Pacific/Honolulu'),
   ('Hongkong', 'Asia/Hong_Kong'),
   ('Iceland', 'Africa/Abidjan'),
   ('Indian/Antananarivo', 'Africa/Nairobi'),
   ('Indian/Christmas', 'Asia/Bangkok'),
   ('Indian/Cocos', 'Asia/Yangon'),
   ('Indian/Comoro', 'Africa/Nairobi'),
   ('Indian/Kerguelen', 'Indian/Maldives'),
   ('Indian/Mahe', 'Asia/Dubai'),
   ('Indian/Mayotte', 'Africa/Nairobi'),
   ('Indian/Reunion', 'Asia/Dubai'),
   ('Iran', 'Asia/Tehran'),
   ('Israel', 'Asia/Jerusalem'),
   ('Jamaica', 'America/Jamaica'),
   ('Japan', 'Asia/Tokyo'),
   ('Kwajalein', 'Pacific/Kwajalein'),
   ('Libya', 'Africa/Tripoli'),
   ('MET', 'Europe/Brussels'),
   ('MST', 'America/Phoenix'),
   ('MST7MDT', 'America/Denver'),
   ('Mexico/BajaNorte', 'America/Tijuana'),
   ('Mexico/BajaSur', 'America/Mazatlan'),
   ('Mexico/General', 'America/Mexico_City'),
   ('NZ', 'Pacific/Auckland'),
   ('NZ-CHAT', 'Pacific/Chatham'),
   ('Navajo', 'America/Denver'),
   ('PRC', 'Asia/Shanghai'),
   ('PST8PDT', 'America/Los_Angeles'),
   ('Pacific/Chuuk', 'Pacific/Port_Moresby'),
   ('Pacific/Enderbury', 'Pacific/Kanton'),
   ('Pacific/Funafuti', 'Pacific/Tarawa'),
   ('Pacific/Johnston', 'Pacific/Honolulu'),
   ('Pacific/Majuro', 'Pacific/Tarawa'),
   ('Pacific/Midway', 'Pacific/Pago_Pago'),
   ('Pacific/Pohnpei', 'Pacific/Guadalcanal'),
   ('Pacific/Ponape', 'Pacific/Guadalcanal'),
   ('Pacific/Saipan', 'Pacific/Guam'),
   ('Pacific/Samoa', 'Pacific/Pago_Pago'),
   ('Pacific/Truk', 'Pacific/Port_Moresby'),
   ('Pacific/Wake', 'Pacific/Tarawa'),
   ('Pacific/Wallis', 'Pacific/Tarawa'),
   ('Pacific/Yap', 'Pacific/Port_Moresby'),
   ('Poland', 'Europe/Warsaw'),
   ('Portugal', 'Europe/Lisbon'),
   ('ROC', 'Asia/Taipei'),
   ('ROK', 'Asia/Seoul'),
   ('Singapore', 'Asia/Singapore'),
   ('Turkey', 'Europe/Istanbul'),
   ('UCT', 'UTC'),
   ('US/Alaska', 'America/Anchorage'),
   ('US/Aleutian', 'America/Adak'),
   ('US/Arizona', 'America/Phoenix'),
   ('US/Central', 'America/Chicago'),
   ('US/East-Indiana', 'America/Indiana/Indianapolis'),
   ('US/Eastern', 'America/New_York'),
   ('US/Hawaii', 'Pacific/Honolulu'),
   ('US/Indiana-Starke', 'America/Indiana/Knox'),
   ('US/Michigan', 'America/Detroit'),
   ('US/Mountain', 'America/Denver'),
   ('US/Pacific', 'America/Los_Angeles'),
   ('US/Samoa', 'Pacific/Pago_Pago'),
   ('Universal', 'UTC'),
   ('W-SU', 'Europe/Moscow'),
   ('WET', 'Europe/Lisbon'),
   ('Z', 'UTC'),
   ('Zulu', 'UTC')
 ) AS links(link,target) WHERE link=z),z)
$$;
UPDATE households SET tzid=pl_storage_tzid(tzid) WHERE tzid IS DISTINCT FROM pl_storage_tzid(tzid);
UPDATE household_members SET tzid=pl_storage_tzid(tzid) WHERE tzid IS DISTINCT FROM pl_storage_tzid(tzid);
UPDATE items SET tzid=pl_storage_tzid(tzid) WHERE tzid IS DISTINCT FROM pl_storage_tzid(tzid);

CREATE OR REPLACE FUNCTION pl_valid_tzid(z text) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT z = 'UTC' OR (z LIKE '%/%' AND z !~ '^(posix|right)/' AND EXISTS(SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name=z))
$$;
-- No NOT VALID escape: name every non-conforming row immediately, including soft deletes.
DO $$
DECLARE bad text;
BEGIN
 SELECT string_agg(relation || ' id=' || id || ' tzid=' || tzid, '; ' ORDER BY relation,id)
 INTO bad FROM (
   SELECT 'households' AS relation,id::text,tzid FROM households
   UNION ALL SELECT 'household_members',id::text,tzid FROM household_members
   UNION ALL SELECT 'items',id::text,tzid FROM items WHERE tzid IS NOT NULL
 ) AS zones WHERE NOT pl_valid_tzid(tzid);
 IF bad IS NOT NULL THEN
   RAISE EXCEPTION 'R35b non-conforming stored time zones: %',bad;
 END IF;
END
$$;
DROP FUNCTION pl_storage_tzid(text);

-- One authoritative ACL, shared by reads, search, conflicts and streams. Agents bypass
-- at their authenticated API boundary, not through a caller-controlled SQL flag.
CREATE FUNCTION pl_item_can_read(p_item uuid, p_member uuid) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT EXISTS (
 SELECT 1 FROM items i JOIN household_members m ON m.household_id=i.household_id AND m.id=p_member
 WHERE i.id=p_item AND i.deleted_at IS NULL AND m.deleted_at IS NULL AND m.role<>'dependent' AND (
 i.visibility='household' OR i.owner_id=p_member
 OR EXISTS(SELECT 1 FROM item_assignees a WHERE a.item_id=i.id AND a.member_id=p_member)
 OR (i.visibility='selected' AND EXISTS(SELECT 1 FROM item_viewers v WHERE v.item_id=i.id AND v.member_id=p_member))))
$$;
GRANT EXECUTE ON FUNCTION pl_item_can_read(uuid,uuid) TO porchlight_app, porchlight_agent;

-- R34: the single household board projection. ACL/scope filters are applied by readers.
-- SET search_path intentionally prevents inlining; materialization is acceptable at household scale.
CREATE FUNCTION pl_board_cards(p_household uuid, p_now timestamptz)
RETURNS SETOF item_occurrences LANGUAGE sql STABLE
SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT o.* FROM item_occurrences o JOIN items i ON i.id=o.item_id
 WHERE o.household_id=p_household AND i.deleted_at IS NULL
   AND o.disposition<>'expired' AND (
   (o.status NOT IN ('done','skipped') AND NOT EXISTS(
     SELECT 1 FROM item_occurrences earlier WHERE earlier.item_id=o.item_id
       AND earlier.status NOT IN ('done','skipped') AND earlier.disposition<>'expired'
       AND (earlier.seq,earlier.id)<(o.seq,o.id)))
   OR (o.status='done' AND o.completed_at>=p_now-interval '7 days'))
$$;
GRANT EXECUTE ON FUNCTION pl_board_cards(uuid,timestamptz) TO porchlight_app,porchlight_agent;

-- Exact calendar interval decomposition shared by derivation and wire projections.
CREATE FUNCTION pl_interval_json(value interval) RETURNS jsonb
LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog,porchlight,pg_temp AS $$
 SELECT jsonb_build_object(
   'months',(extract(year FROM value)*12+extract(month FROM value))::int,
   'days',extract(day FROM value)::int,
   'microseconds',(extract(hour FROM value)*3600000000+
     extract(minute FROM value)*60000000+extract(second FROM value)*1000000)::bigint)
$$;
GRANT EXECUTE ON FUNCTION pl_interval_json(interval) TO porchlight_app,porchlight_agent;

-- simple preserves multilingual household vocabulary/names rather than English stemming.
ALTER TABLE items ADD COLUMN search_document tsvector GENERATED ALWAYS AS
 (to_tsvector('simple'::regconfig, coalesce(title,'') || ' ' || coalesce(notes,'') || ' ' || coalesce(address,''))) STORED;
CREATE INDEX items_search_document ON items USING gin(search_document);

-- Derived search tokens are not authoritative revision fields or business changes.
CREATE OR REPLACE FUNCTION pl_item_snapshot(p_item_id uuid) RETURNS jsonb
    LANGUAGE sql STABLE
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
    SELECT (to_jsonb(i)-'search_document') || jsonb_build_object(
        'assignees', coalesce((SELECT jsonb_agg(a.member_id ORDER BY a.member_id) FROM item_assignees a WHERE a.item_id = i.id), '[]'),
        'viewers', coalesce((SELECT jsonb_agg(v.member_id ORDER BY v.member_id) FROM item_viewers v WHERE v.item_id = i.id), '[]'),
        'reminders', coalesce((SELECT jsonb_agg(r.offset_minutes ORDER BY r.offset_minutes) FROM item_reminders r WHERE r.item_id = i.id), '[]'))
    FROM items i WHERE i.id = p_item_id
$$;

CREATE OR REPLACE FUNCTION pl_items_audit() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    who jsonb := pl_current_actor();
BEGIN
    INSERT INTO item_revisions (household_id, item_id, rev, snapshot, actor)
    SELECT n.household_id, n.id,
           coalesce((SELECT max(r.rev) FROM item_revisions r WHERE r.item_id = n.id), 0) + 1,
           pl_item_snapshot(n.id), who
    FROM new_rows n;

    IF TG_OP = 'INSERT' THEN
        INSERT INTO task_events (household_id, item_id, kind, actor)
        SELECT household_id, id, 'created', who FROM new_rows;
    ELSE
        INSERT INTO task_events (household_id, item_id, kind, actor, data)
        SELECT c.household_id, c.id, c.kind, who,
               CASE WHEN c.kind = 'updated' THEN jsonb_build_object('changed', c.changed) ELSE '{}'::jsonb END
        FROM (
            SELECT n.household_id, n.id,
                   CASE WHEN o.deleted_at IS NULL AND n.deleted_at IS NOT NULL THEN 'deleted'
                        WHEN o.deleted_at IS NOT NULL AND n.deleted_at IS NULL THEN 'restored'
                        ELSE 'updated' END AS kind,
                   (SELECT coalesce(jsonb_agg(e.key ORDER BY e.key), '[]')
                    FROM jsonb_each(to_jsonb(n)) e
                    WHERE e.key NOT IN ('version', 'updated_at', 'search_document')
                      AND to_jsonb(o) -> e.key IS DISTINCT FROM e.value) AS changed
            FROM new_rows n JOIN old_rows o ON o.id = n.id
        ) c
        WHERE c.kind <> 'updated' OR c.changed <> '[]'::jsonb;
    END IF;
    RETURN NULL;
END $$;



CREATE FUNCTION pl_settings_outbox() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
 VALUES(NEW.household_id,'settings.changed','member_settings',NEW.member_id,jsonb_build_object('member_id',NEW.member_id));
 PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;
CREATE TRIGGER settings_outbox AFTER INSERT OR UPDATE ON member_settings FOR EACH ROW EXECUTE FUNCTION pl_settings_outbox();

-- R53: notification kinds are extensible data, never a closed CHECK/function list.
CREATE TABLE notification_kinds (
 kind text PRIMARY KEY,
 self_suppressed boolean NOT NULL,
 description text NOT NULL
);
INSERT INTO notification_kinds(kind,self_suppressed,description) VALUES
 ('assigned',true,'Item assignment changed'),
 ('status_changed',true,'Occurrence status changed'),
 ('completed',true,'Occurrence completed'),
 ('comment',true,'Comment added'),
 ('mention',true,'Member mentioned'),
 ('conflict',false,'Hard scheduling conflict'),
 ('load_conflict',false,'Availability load conflict'),
 ('reminder',false,'Scheduled reminder'),
 ('dependent_unassigned',true,'Dependent item needs an assignee'),
 ('derivation_error',false,'Item derivation failed'),
 ('suggestion',true,'Scheduling suggestion');
GRANT SELECT ON notification_kinds TO porchlight_app,porchlight_agent;

CREATE TABLE notification_events (
 id uuid PRIMARY KEY DEFAULT uuidv7(),
 household_id uuid NOT NULL REFERENCES households(id),
 recipient_member_id uuid NOT NULL,
 kind text NOT NULL REFERENCES notification_kinds(kind),
 item_id uuid, occurrence_id uuid,
 entity_type text NOT NULL CHECK(length(entity_type) BETWEEN 1 AND 64),
 entity_id uuid NOT NULL,
 actor_member_id uuid, actor_label text,
 payload jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(payload)='object'),
 dedupe_key text,
 created_at timestamptz NOT NULL DEFAULT now(), processed_at timestamptz,
 UNIQUE(household_id,dedupe_key),
 FOREIGN KEY(household_id,recipient_member_id) REFERENCES household_members(household_id,id),
 FOREIGN KEY(household_id,actor_member_id) REFERENCES household_members(household_id,id),
 -- No occurrence FK: append-only events must not prevent occurrence regeneration.
 FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id)
);
CREATE INDEX notification_events_unprocessed ON notification_events(created_at,id) WHERE processed_at IS NULL;
CREATE FUNCTION pl_notification_events_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF TG_OP='DELETE' THEN
   IF current_user='porchlight_maintenance' THEN RETURN OLD; END IF;
   RAISE EXCEPTION 'notification events are append-only' USING ERRCODE='PL403';
 END IF;
 IF (to_jsonb(NEW)-'processed_at') IS DISTINCT FROM (to_jsonb(OLD)-'processed_at')
 OR (OLD.processed_at IS NOT NULL AND NEW.processed_at IS DISTINCT FROM OLD.processed_at) THEN
 RAISE EXCEPTION 'notification events are immutable' USING ERRCODE='PL403'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER notification_events_immutable BEFORE UPDATE OR DELETE ON notification_events FOR EACH ROW EXECUTE FUNCTION pl_notification_events_immutable();

-- NULL means suppressed or a duplicate; producers must never rely on an insertion.
CREATE FUNCTION pl_emit_notification_event(
 p_household uuid, p_recipient uuid, p_kind text, p_entity_type text, p_entity_id uuid,
 p_payload jsonb DEFAULT '{}', p_item uuid DEFAULT NULL, p_occurrence uuid DEFAULT NULL, p_dedupe_key text DEFAULT NULL
) RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE a jsonb := pl_current_actor(); member uuid := (a->>'member_id')::uuid; result uuid;
BEGIN
 IF p_occurrence IS NOT NULL AND NOT EXISTS(
   SELECT 1 FROM item_occurrences WHERE id=p_occurrence AND household_id=p_household
   AND (p_item IS NULL OR item_id=p_item)
 ) THEN RAISE EXCEPTION 'invalid occurrence reference' USING ERRCODE='23503'; END IF;
 IF member=p_recipient AND (SELECT self_suppressed FROM notification_kinds WHERE kind=p_kind) THEN RETURN NULL; END IF;
 INSERT INTO notification_events(household_id,recipient_member_id,kind,item_id,occurrence_id,entity_type,entity_id,actor_member_id,actor_label,payload,dedupe_key)
 VALUES(p_household,p_recipient,p_kind,p_item,p_occurrence,p_entity_type,p_entity_id,member,a->>'application_name',p_payload,p_dedupe_key)
 ON CONFLICT(household_id,dedupe_key) DO NOTHING RETURNING id INTO result;
 RETURN result;
END $$;
REVOKE ALL ON FUNCTION pl_emit_notification_event(uuid,uuid,text,text,uuid,jsonb,uuid,uuid,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_emit_notification_event(uuid,uuid,text,text,uuid,jsonb,uuid,uuid,text) TO porchlight_app,porchlight_agent;
GRANT SELECT ON notification_events TO porchlight_app,porchlight_agent;
GRANT UPDATE(processed_at) ON notification_events TO porchlight_app;
-- Every profile starts with a versioned settings resource; GET never creates rows.
CREATE FUNCTION pl_member_settings_seed() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 INSERT INTO member_settings(household_id,member_id,settings) VALUES(NEW.household_id,NEW.id,
 '{"today_sections":["overdue","today","this_week","needs_owner"],"scope":"mine","this_week_days":7,"pinned_list_id":null}');
 RETURN NULL;
END $$;
CREATE TRIGGER member_settings_seed AFTER INSERT ON household_members FOR EACH ROW EXECUTE FUNCTION pl_member_settings_seed();
INSERT INTO member_settings(household_id,member_id,settings)
SELECT household_id,id,'{"today_sections":["overdue","today","this_week","needs_owner"],"scope":"mine","this_week_days":7,"pinned_list_id":null}'::jsonb FROM household_members
ON CONFLICT(member_id) DO NOTHING;
RESET search_path;
RESET ROLE;
