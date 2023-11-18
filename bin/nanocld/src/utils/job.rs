use std::collections::HashMap;

use ntex::util::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use futures_util::stream::{FuturesUnordered, select_all, FuturesOrdered};
use bollard_next::service::{
  ContainerSummary, ContainerInspectResponse, ContainerWaitExitError,
};
use bollard_next::container::{
  CreateContainerOptions, StartContainerOptions, ListContainersOptions,
  RemoveContainerOptions, LogsOptions, WaitContainerOptions,
};

use nanocl_error::http::HttpError;
use nanocl_stubs::node::NodeContainerSummary;
use nanocl_stubs::job::{
  Job, JobPartial, JobInspect, JobLogOutput, JobWaitResponse, WaitCondition,
  JobSummary,
};

use crate::repositories;
use crate::models::{DaemonState, JobUpdateDbModel};

use super::stream::transform_stream;

/// ## List instances
///
/// List the job instances (containers) based on the job name
///
/// ## Arguments
///
/// * [name](str) - The job name
/// * [docker_api](bollard_next::Docker) - The docker api
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [Vector](Vec) of [ContainerSummary](ContainerSummary)
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn list_instances(
  name: &str,
  docker_api: &bollard_next::Docker,
) -> Result<Vec<ContainerSummary>, HttpError> {
  let label = format!("io.nanocl.job={name}");
  let mut filters: HashMap<&str, Vec<&str>> = HashMap::new();
  filters.insert("label", vec![&label]);
  let options = Some(ListContainersOptions {
    all: true,
    filters,
    ..Default::default()
  });
  let containers = docker_api.list_containers(options).await?;
  Ok(containers)
}

/// ## Inspect instances
///
/// Return detailed informations about each instances of a job
///
/// ## Arguments
///
/// [name](str) The job name
/// [state](DaemonState) The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [Vector](Vec) of [ContainerInspectResponse](ContainerInspectResponse) and [NodeContainerSummary](NodeContainerSummary)
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
async fn inspect_instances(
  name: &str,
  state: &DaemonState,
) -> Result<Vec<(ContainerInspectResponse, NodeContainerSummary)>, HttpError> {
  list_instances(name, &state.docker_api).await?
  .into_iter()
  .map(|container| async {
    let container_inspect = state
      .docker_api
      .inspect_container(&container.id.clone().unwrap_or_default(), None)
      .await?;
    Ok::<_, HttpError>((
      container_inspect,
      NodeContainerSummary {
        node: state.config.hostname.clone(),
        ip_address: state.config.advertise_addr.clone(),
        container,
      },
    ))
  })
  .collect::<FuturesUnordered<_>>()
  .collect::<Vec<Result<(ContainerInspectResponse, NodeContainerSummary), _>>>()
  .await.into_iter().collect::<Result<Vec<(ContainerInspectResponse, NodeContainerSummary)>, _>>()
}

/// ## Count instances
///
/// Count the number of instances (containers) of a job
///
/// ## Arguments
///
/// * [instances](Vec) - Instances of [ContainerInspectResponse](ContainerInspectResponse) and [NodeContainerSummary](NodeContainerSummary)
/// * [state](DaemonState) - The daemon state
///
/// ## Return
///
/// * [Tuple](Tuple) - The tuple of the number of instances
///   * [usize] - The total number of instances
///   * [usize] - The number of failed instances
///   * [usize] - The number of success instances
///   * [usize] - The number of running instances
///
fn count_instances(
  instances: &[(ContainerInspectResponse, NodeContainerSummary)],
) -> (usize, usize, usize, usize) {
  let mut instance_failed = 0;
  let mut instance_success = 0;
  let mut instance_running = 0;
  for (container_inspect, _) in instances {
    let state = container_inspect.state.clone().unwrap_or_default();
    if state.running.unwrap_or_default() {
      instance_running += 1;
      continue;
    }
    if let Some(exit_code) = state.exit_code {
      if exit_code == 0 {
        instance_success += 1;
      } else {
        instance_failed += 1;
      }
    }
    if let Some(error) = state.error {
      if !error.is_empty() {
        instance_failed += 1;
      }
    }
  }
  (
    instances.len(),
    instance_failed,
    instance_success,
    instance_running,
  )
}

/// ## Create
///
/// Create a job and run it
///
/// ## Arguments
///
/// * [item](JobPartial) - The job partial
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [Job](Job) has been created
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn create(
  item: &JobPartial,
  state: &DaemonState,
) -> Result<Job, HttpError> {
  let job = repositories::job::create(item, &state.pool).await?;
  job
    .containers
    .iter()
    .map(|container| {
      let job_name = job.name.clone();
      async move {
        let mut container = container.clone();
        let mut labels = container.labels.clone().unwrap_or_default();
        labels.insert("io.nanocl.job".to_owned(), job_name.clone());
        container.labels = Some(labels);
        state
          .docker_api
          .create_container(
            None::<CreateContainerOptions<String>>,
            container.clone(),
          )
          .await?;
        Ok::<_, HttpError>(())
      }
    })
    .collect::<FuturesUnordered<_>>()
    .collect::<Vec<Result<(), HttpError>>>()
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
  Ok(job)
}

/// ## Start by name
///
/// Start a job by name
///
/// ## Arguments
///
/// * [name](str) - The job name
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - The job has been started
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn start_by_name(
  name: &str,
  state: &DaemonState,
) -> Result<(), HttpError> {
  repositories::job::find_by_name(name, &state.pool).await?;
  let containers = inspect_instances(name, state).await?;
  containers
    .into_iter()
    .map(|(inspect, _)| async {
      if inspect
        .state
        .unwrap_or_default()
        .running
        .unwrap_or_default()
      {
        return Ok(());
      }
      state
        .docker_api
        .start_container(
          &inspect.id.unwrap_or_default(),
          None::<StartContainerOptions<String>>,
        )
        .await?;
      Ok::<_, HttpError>(())
    })
    .collect::<FuturesOrdered<_>>()
    .collect::<Vec<Result<(), HttpError>>>()
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
  repositories::job::update_by_name(
    name,
    &JobUpdateDbModel {
      updated_at: Some(chrono::Utc::now().naive_utc()),
    },
    &state.pool,
  )
  .await?;
  Ok(())
}

/// ## List
///
/// List all jobs
///
/// ## Arguments
///
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [Vector](Vec) of [Job](JobSummary)
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn list(state: &DaemonState) -> Result<Vec<JobSummary>, HttpError> {
  let jobs = repositories::job::list(&state.pool).await?;
  let job_summaries =
    jobs
      .iter()
      .map(|job| async {
        let instances = inspect_instances(&job.name, state).await?;
        let (
          instance_total,
          instance_failed,
          instance_success,
          instance_running,
        ) = count_instances(&instances);
        Ok::<_, HttpError>(JobSummary {
          name: job.name.clone(),
          created_at: job.created_at,
          updated_at: job.updated_at,
          config: job.clone(),
          instance_total,
          instance_success,
          instance_running,
          instance_failed,
        })
      })
      .collect::<FuturesUnordered<_>>()
      .collect::<Vec<Result<JobSummary, HttpError>>>()
      .await
      .into_iter()
      .collect::<Result<Vec<JobSummary>, HttpError>>()?;
  Ok(job_summaries)
}

/// ## Delete by name
///
/// Delete a job by key with his given instances (containers).
///
/// ## Arguments
///
/// * [key](str) - The job key
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - The job has been deleted
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn delete_by_name(
  name: &str,
  state: &DaemonState,
) -> Result<(), HttpError> {
  let job = repositories::job::find_by_name(name, &state.pool).await?;
  let containers = list_instances(name, &state.docker_api).await?;
  containers
    .into_iter()
    .map(|container| async {
      state
        .docker_api
        .remove_container(
          &container.id.unwrap_or_default(),
          Some(RemoveContainerOptions {
            force: true,
            ..Default::default()
          }),
        )
        .await
        .map_err(HttpError::from)
    })
    .collect::<FuturesUnordered<_>>()
    .collect::<Vec<Result<(), HttpError>>>()
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
  repositories::job::delete_by_name(&job.name, &state.pool).await?;
  Ok(())
}

/// ## Inspect by name
///
/// Inspect a job by name and return a detailed view of the job
///
/// ## Arguments
///
/// * [name](str) - The job name
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [JobInspect](JobInspect) has been returned
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn inspect_by_name(
  name: &str,
  state: &DaemonState,
) -> Result<JobInspect, HttpError> {
  let job = repositories::job::find_by_name(name, &state.pool).await?;
  let instances = inspect_instances(name, state).await?;
  let (instance_total, instance_failed, instance_success, instance_running) =
    count_instances(&instances);
  let job_inspect = JobInspect {
    name: job.name,
    created_at: job.created_at,
    updated_at: job.updated_at,
    secrets: job.secrets,
    metadata: job.metadata,
    containers: job.containers,
    instance_total,
    instance_success,
    instance_running,
    instance_failed,
    instances: instances
      .clone()
      .into_iter()
      .map(|(_, container)| container)
      .collect(),
  };
  Ok(job_inspect)
}

/// ## Logs by name
///
/// Get the logs of a job by name
///
/// ## Arguments
///
/// * [name](str) - The job name
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Ok) - [Stream](StreamExt) of [JobLogOutput](JobLogOutput)
///   * [Err](Err) - [Http error](HttpError) Something went wrong
///
pub async fn logs_by_name(
  name: &str,
  state: &DaemonState,
) -> Result<impl StreamExt<Item = Result<Bytes, HttpError>>, HttpError> {
  let _ = repositories::job::find_by_name(name, &state.pool).await?;
  let instances = list_instances(name, &state.docker_api).await?;
  let futures = instances
    .into_iter()
    .map(|instance| {
      state
        .docker_api
        .logs(
          &instance.id.unwrap_or_default(),
          Some(LogsOptions::<String> {
            stdout: true,
            ..Default::default()
          }),
        )
        .map(move |elem| match elem {
          Err(err) => Err(err),
          Ok(elem) => Ok(JobLogOutput {
            container_name: instance
              .names
              .clone()
              .unwrap_or_default()
              .join("")
              .replace('/', ""),
            log: elem.into(),
          }),
        })
    })
    .collect::<Vec<_>>();
  let stream = select_all(futures).into_stream();
  Ok(transform_stream::<JobLogOutput, JobLogOutput>(stream))
}

/// ## Wait
///
/// Wait a job to finish
/// And create his instances (containers).
///
/// ## Arguments
///
/// * [key](str) - The job key
/// * [state](DaemonState) - The daemon state
///
/// ## Returns
///
/// * [Result](Result) - The result of the operation
///   * [Ok](Stream) - The stream of wait
///   * [Err](HttpError) - The job cannot be waited
///
pub async fn wait(
  name: &str,
  wait_options: WaitContainerOptions<WaitCondition>,
  state: &DaemonState,
) -> Result<impl StreamExt<Item = Result<Bytes, HttpError>>, HttpError> {
  let job = repositories::job::find_by_name(name, &state.pool).await?;
  let docker_api = state.docker_api.clone();
  let containers = list_instances(&job.name, &docker_api).await?;
  let mut streams = Vec::new();
  for container in containers {
    let id = container.id.unwrap_or_default();
    let options = Some(wait_options.clone());
    let container_name = container
      .names
      .clone()
      .unwrap_or_default()
      .join("")
      .replace('/', "");
    let stream =
      docker_api
        .wait_container(&id, options)
        .map(move |wait_result| match wait_result {
          Err(err) => {
            if let bollard_next::errors::Error::DockerContainerWaitError {
              error,
              code,
            } = &err
            {
              return Ok(JobWaitResponse {
                container_name: container_name.clone(),
                status_code: *code,
                error: Some(ContainerWaitExitError {
                  message: Some(error.to_owned()),
                }),
              });
            }
            Err(err)
          }
          Ok(wait_response) => {
            Ok(JobWaitResponse::from_container_wait_response(
              wait_response,
              container_name.clone(),
            ))
          }
        });
    streams.push(stream);
  }
  let stream = select_all(streams).into_stream();
  Ok(transform_stream::<JobWaitResponse, JobWaitResponse>(stream))
}