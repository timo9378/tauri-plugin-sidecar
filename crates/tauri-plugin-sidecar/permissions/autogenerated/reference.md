## Default Permission

Allows reading sidecar status and tailing logs. Mutating commands (start/stop/restart) must be granted explicitly.

#### This default permission set includes the following:

- `allow-status`
- `allow-logs`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`sidecar:allow-logs`

</td>
<td>

Enables the logs command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:deny-logs`

</td>
<td>

Denies the logs command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:allow-restart`

</td>
<td>

Enables the restart command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:deny-restart`

</td>
<td>

Denies the restart command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:allow-start`

</td>
<td>

Enables the start command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:deny-start`

</td>
<td>

Denies the start command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:allow-status`

</td>
<td>

Enables the status command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:deny-status`

</td>
<td>

Denies the status command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:allow-stop`

</td>
<td>

Enables the stop command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sidecar:deny-stop`

</td>
<td>

Denies the stop command without any pre-configured scope.

</td>
</tr>
</table>
