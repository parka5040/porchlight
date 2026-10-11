-- A pure, inlineable expression: callers retain their authoritative joins and ACLs.
-- No table reads or stored state; existing role/member/household-zone enqueue triggers apply.
SET LOCAL ROLE porchlight_owner;
CREATE FUNCTION porchlight.pl_effective_member_zone(member_role text, member_zone text, household_zone text)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN CASE WHEN member_role = 'dependent' THEN household_zone ELSE member_zone END;
REVOKE ALL ON FUNCTION porchlight.pl_effective_member_zone(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION porchlight.pl_effective_member_zone(text,text,text)
 TO porchlight_app, porchlight_agent, porchlight_maintenance, porchlight_migrator;

RESET ROLE;
