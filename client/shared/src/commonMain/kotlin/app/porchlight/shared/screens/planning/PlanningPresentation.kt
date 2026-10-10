package app.porchlight.shared.screens.planning

import app.porchlight.shared.api.models.*
import kotlin.time.Instant

/** Uses the device locale, but always the selected member's calendar zone. */
expect fun formatPlanningTime(value: Instant, zone: String): String
fun optionalPlanningTime(value: Instant?, zone: String) = value?.let { formatPlanningTime(it, zone) } ?: "never"
fun unplacedReason(reason: UnplacedReason): String = when (reason) {
    UnplacedReason.unknown_member -> "Member no longer available"
    UnplacedReason.invalid_estimate -> "Add a valid time estimate"
    UnplacedReason.invalid_deadline -> "Add a valid deadline"
    UnplacedReason.no_slot_before_deadline -> "No available time before the deadline"
    UnplacedReason.duplicate_task -> "Work already included in this run"
}
fun candidateReason(reason: CandidateReason): String = when (reason) {
    CandidateReason.free_inside_availability -> "Free during available hours"
    CandidateReason.free_outside_availability -> "Free, outside available hours"
    CandidateReason.conflicting -> "Overlaps scheduled work"
    CandidateReason.invalid_appointment -> "Appointment timing needs correction"
}
// MemberRead.tzid already resolves dependents to the household zone on the server.
fun planningMemberZone(member: MemberRead?) = member?.tzid ?: "UTC"
fun defaultFeedVisibility(target: MemberRead?) =
    if (target == null || target.role == "dependent") SourceVisibility.household else SourceVisibility.private
