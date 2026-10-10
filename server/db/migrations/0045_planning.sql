SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Empty is the default. The profile versions the complete window collection.
CREATE TABLE availability_profiles (
    member_id uuid PRIMARY KEY, household_id uuid NOT NULL, version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(household_id,member_id) REFERENCES household_members(household_id,id)
);
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON availability_profiles
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('member_id','household_id');
CREATE TABLE availability_windows (
    id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, member_id uuid NOT NULL,
    weekday smallint NOT NULL CHECK(weekday BETWEEN 0 AND 6), -- Monday=0
    starts_local time NOT NULL CHECK(starts_local < '24:00'),
    ends_local time NOT NULL CHECK(ends_local < '24:00'),
    version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(household_id,member_id) REFERENCES household_members(household_id,id),
    UNIQUE(member_id,weekday,starts_local,ends_local)
);
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON availability_windows
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','member_id');
CREATE TABLE schedule_suggestions (
    id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, member_id uuid NOT NULL,
    item_id uuid NOT NULL, occurrence_id uuid NOT NULL, proposed_start timestamptz NOT NULL,
    proposed_end timestamptz NOT NULL, status text NOT NULL DEFAULT 'proposed' CHECK(status IN ('proposed','accepted','dismissed')),
    run_id uuid NOT NULL, version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK(proposed_start < proposed_end),
    FOREIGN KEY(household_id,member_id) REFERENCES household_members(household_id,id),
    FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id)
    -- No occurrence FK: a materialized occurrence can disappear during regeneration.
);
CREATE INDEX schedule_suggestions_member ON schedule_suggestions(member_id,status,id);
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON schedule_suggestions
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','member_id','item_id','occurrence_id','run_id','proposed_start','proposed_end');
CREATE TABLE planning_audit (
    id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, entity_type text NOT NULL,
    entity_id uuid NOT NULL, operation text NOT NULL, snapshot jsonb NOT NULL, actor jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE member_load_jobs (
    member_id uuid PRIMARY KEY REFERENCES household_members(id), requested_gen bigint NOT NULL DEFAULT 1,
    applied_gen bigint NOT NULL DEFAULT 0, updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE member_daily_load (
    member_id uuid NOT NULL REFERENCES household_members(id), date date NOT NULL,
    planned_minutes bigint NOT NULL CHECK(planned_minutes >= 0), available_minutes bigint NOT NULL CHECK(available_minutes >= 0),
    starts_at timestamptz NOT NULL, ends_at timestamptz NOT NULL, derived_at timestamptz NOT NULL,
    PRIMARY KEY(member_id,date), CHECK(starts_at<ends_at)
);

CREATE FUNCTION pl_enqueue_member_load(p_member uuid) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO member_load_jobs(member_id) VALUES(p_member)
    ON CONFLICT(member_id) DO UPDATE SET requested_gen=member_load_jobs.requested_gen+1,updated_at=now();
    PERFORM pg_notify('porchlight_derive','');
END $$;
CREATE FUNCTION pl_availability_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record; who jsonb := pl_current_actor();
BEGIN
    IF TG_OP='DELETE' THEN r := OLD; ELSE r := NEW; END IF;
    INSERT INTO availability_profiles(member_id,household_id) VALUES(r.member_id,r.household_id)
    ON CONFLICT(member_id) DO UPDATE SET updated_at=now();
    INSERT INTO planning_audit(household_id,entity_type,entity_id,operation,snapshot,actor)
        VALUES(r.household_id,'availability',r.member_id,TG_OP,to_jsonb(r),who);
    PERFORM pl_enqueue_member_load(r.member_id);
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        VALUES(r.household_id,'member.changed','availability',r.member_id,jsonb_build_object('member_id',r.member_id));
    PERFORM pg_notify('porchlight_outbox','');
    RETURN NULL;
END $$;
CREATE TRIGGER availability_changed AFTER INSERT OR UPDATE OR DELETE ON availability_windows
    FOR EACH ROW EXECUTE FUNCTION pl_availability_changed();
CREATE FUNCTION pl_planning_member_zone() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN PERFORM pl_enqueue_member_load(NEW.id); RETURN NULL; END $$;
CREATE TRIGGER planning_member_zone AFTER UPDATE OF tzid ON household_members FOR EACH ROW
    WHEN(OLD.tzid IS DISTINCT FROM NEW.tzid) EXECUTE FUNCTION pl_planning_member_zone();
CREATE FUNCTION pl_suggestion_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    IF TG_OP='DELETE' THEN r:=OLD; ELSE r:=NEW; END IF;
    IF TG_OP='INSERT' AND NOT EXISTS(SELECT 1 FROM item_occurrences o WHERE o.id=r.occurrence_id AND o.item_id=r.item_id AND o.household_id=r.household_id) THEN
        RAISE EXCEPTION 'invalid occurrence reference' USING ERRCODE='PL422';
    END IF;
    INSERT INTO planning_audit(household_id,entity_type,entity_id,operation,snapshot,actor)
        VALUES(r.household_id,'suggestion',r.id,TG_OP,to_jsonb(r),pl_current_actor());
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        VALUES(r.household_id,'suggestion.changed','suggestion',r.id,jsonb_build_object('member_id',r.member_id));
    PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;
CREATE TRIGGER suggestion_changed AFTER INSERT OR UPDATE OR DELETE ON schedule_suggestions
    FOR EACH ROW EXECUTE FUNCTION pl_suggestion_changed();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON availability_windows
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON schedule_suggestions
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER planning_audit_immutable BEFORE UPDATE OR DELETE ON planning_audit
    FOR EACH ROW EXECUTE FUNCTION pl_append_only();


-- Collection-level writes (including empty→empty) retain attribution and tickles.
CREATE FUNCTION pl_availability_profile_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO planning_audit(household_id,entity_type,entity_id,operation,snapshot,actor)
        VALUES(NEW.household_id,'availability',NEW.member_id,TG_OP,to_jsonb(NEW),pl_current_actor());
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        VALUES(NEW.household_id,'member.changed','availability',NEW.member_id,jsonb_build_object('member_id',NEW.member_id));
    PERFORM pg_notify('porchlight_outbox','');RETURN NULL;
END $$;
CREATE TRIGGER availability_profile_changed AFTER INSERT OR UPDATE ON availability_profiles
    FOR EACH ROW EXECUTE FUNCTION pl_availability_profile_changed();
REVOKE ALL ON FUNCTION pl_availability_profile_changed() FROM PUBLIC;

REVOKE ALL ON FUNCTION pl_enqueue_member_load(uuid),pl_availability_changed(),pl_planning_member_zone(),pl_suggestion_changed() FROM PUBLIC;
GRANT SELECT ON availability_profiles,availability_windows,schedule_suggestions,planning_audit,member_load_jobs,member_daily_load TO porchlight_app,porchlight_agent;
GRANT INSERT(member_id,household_id),UPDATE(updated_at) ON availability_profiles TO porchlight_app;
GRANT INSERT(household_id,member_id,weekday,starts_local,ends_local),UPDATE(weekday,starts_local,ends_local),DELETE ON availability_windows TO porchlight_app,porchlight_agent;
GRANT INSERT(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id),UPDATE(status),DELETE ON schedule_suggestions TO porchlight_app;
-- Agents may dismiss/accept (acceptance must also write the occurrence); proposals are generated only.
GRANT UPDATE(status) ON schedule_suggestions TO porchlight_agent;
GRANT INSERT,UPDATE,DELETE ON member_daily_load TO porchlight_app;
GRANT UPDATE(applied_gen) ON member_load_jobs TO porchlight_app;
GRANT EXECUTE ON FUNCTION pl_enqueue_member_load(uuid) TO porchlight_app;
GRANT SELECT,DELETE ON availability_profiles,availability_windows,schedule_suggestions,planning_audit,member_load_jobs,member_daily_load TO porchlight_maintenance;
RESET search_path;
RESET ROLE;
