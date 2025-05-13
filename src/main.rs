use anyhow::Result;
use azure_devops_rust_api::git::{models::GitPullRequest, Client, ClientBuilder};
use std::process::Command;

#[derive(Debug, PartialEq)]
struct AzureRepoComponents {
    org: String,
    project: String,
    repo: String,
}

fn parse_azure_git_url(url: &str) -> Option<AzureRepoComponents> {
    let ssh_pattern =
        regex::Regex::new(r"^git@ssh\.dev\.azure\.com:v3/([^/]+)/([^/]+)/([^/]+)$").unwrap();
    let https_pattern =
        regex::Regex::new(r"^https://([^@]+)@dev\.azure\.com/([^/]+)/([^/]+)/_git/([^/]+)$")
            .unwrap();

    if let Some(caps) = ssh_pattern.captures(url) {
        return Some(AzureRepoComponents {
            org: caps[1].trim().to_string(),
            project: caps[2].trim().to_string(),
            repo: caps[3].trim().to_string(),
        });
    }

    if let Some(caps) = https_pattern.captures(url) {
        return Some(AzureRepoComponents {
            org: caps[2].trim().to_string(),
            project: caps[3].trim().to_string(),
            repo: caps[4].trim().to_string(),
        });
    }

    None
}

fn stdout_str(command: &mut Command) -> Result<String> {
    let stdout = command.output().map(|result| {
        String::from_utf8(result.stdout)
            .expect("output from git remote command should be utf-8 encoded strings")
            .trim()
            .to_string()
    })?;
    Ok(stdout)
}

fn git_origin_url() -> String {
    stdout_str(Command::new("git").args(["remote", "get-url", "origin", "--all"])).expect("git remote url for origin should be acccessable by running git in current working directory")
}

fn git_head() -> String {
    stdout_str(Command::new("git").args(["rev-parse", "HEAD"]))
        .expect("running git in current working directory should provide reference HEAD")
}

struct AchClient {
    org: String,
    project: String,
    repo: String,
    head: String,
    client: Client,
}

struct AchInfo {
    pr: i32,
    work_items: Vec<i32>,
}

impl AchInfo {
    fn display(&self) {
        println!("Pull-request #{}", self.pr);
        for work_item in &self.work_items {
            println!("Work-item #{}", work_item);
        }
    }
}

impl AchClient {
    fn new() -> Self {
        let remote_url = git_origin_url();
        let AzureRepoComponents { org, repo, project } = parse_azure_git_url(&remote_url).expect("parse of remote url should reveal azure repository components org name, repo name and project name");
        let head = git_head();
        let pat = std::env::var("ADO_PAT").expect("environment variable ADO_PAT should be set");
        let credential = azure_devops_rust_api::Credential::Pat(pat);
        let client = ClientBuilder::new(credential).build();
        Self {
            client,
            org,
            project,
            repo,
            head,
        }
    }

    async fn repo_pull_requets(&self) -> Result<Vec<GitPullRequest>> {
        let client = self.client.pull_requests_client();
        Ok(client
            .get_pull_requests(self.org.clone(), self.repo.clone(), self.project.clone())
            .await?
            .value)
    }

    async fn pull_request_commit_ids(&self, pull_request: &GitPullRequest) -> Result<Vec<String>> {
        let client = self.client.pull_request_commits_client();
        let commits = client
            .get_pull_request_commits(
                self.org.clone(),
                pull_request.repository.id.clone(),
                pull_request.pull_request_id,
                self.project.clone(),
            )
            .await?
            .value;

        Ok(commits
            .into_iter()
            .flat_map(|commit| commit.commit_id)
            .collect())
    }

    async fn pull_request_work_item_ids(&self, pull_request: &GitPullRequest) -> Result<Vec<i32>> {
        let client = self.client.pull_request_work_items_client();
        let work_items: Vec<i32> = client
            .list(
                self.org.clone(),
                self.repo.clone(),
                pull_request.pull_request_id,
                self.project.clone(),
            )
            .await?
            .value
            .into_iter()
            .flat_map(|resource_ref| {
                resource_ref.id.map(|id| {
                    id.parse::<i32>()
                        .expect("parse work item id as i32 should work")
                })
            })
            .collect();

        Ok(work_items)
    }

    async fn pull_request(&self) -> Result<Option<GitPullRequest>> {
        for pull_request in self.repo_pull_requets().await? {
            for commit in self.pull_request_commit_ids(&pull_request).await? {
                if commit.eq(&self.head) {
                    return Ok(Some(pull_request));
                }
            }
        }
        Ok(None)
    }

    async fn info(&self) -> Result<Option<AchInfo>> {
        let Some(pull_request) = self.pull_request().await? else {
            return Ok(None);
        };

        // Allow partial success i.e. only PR id
        let work_items = self
            .pull_request_work_item_ids(&pull_request)
            .await
            .unwrap_or(vec![]);

        Ok(Some(AchInfo {
            pr: pull_request.pull_request_id,
            work_items,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let client = AchClient::new();

    match client.info().await? {
        Some(info) => info.display(),
        None => println!("no pr info found"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_ssh_url() {
        let url = "git@ssh.dev.azure.com:v3/MyOrg/MyProject/MyRepo";
        let result = parse_azure_git_url(url);
        assert_eq!(
            result,
            Some(AzureRepoComponents {
                org: "MyOrg".to_string(),
                project: "MyProject".to_string(),
                repo: "MyRepo".to_string()
            })
        );
    }

    #[test]
    fn parses_valid_https_url() {
        let url = "https://MyOrg@dev.azure.com/MyOrg/MyProject/_git/MyRepo";
        let result = parse_azure_git_url(url);
        assert_eq!(
            result,
            Some(AzureRepoComponents {
                org: "MyOrg".to_string(),
                project: "MyProject".to_string(),
                repo: "MyRepo".to_string()
            })
        );
    }

    #[test]
    fn fails_on_malformed_ssh_url() {
        let url = "git@ssh.dev.azure.com:MyOrg/MyProject/MyRepo";
        assert_eq!(parse_azure_git_url(url), None);
    }

    #[test]
    fn fails_on_malformed_https_url() {
        let url = "https://dev.azure.com/MyOrg/MyProject/_git/MyRepo";
        assert_eq!(parse_azure_git_url(url), None);
    }

    #[test]
    fn fails_on_unrelated_url() {
        let url = "https://github.com/user/repo.git";
        assert_eq!(parse_azure_git_url(url), None);
    }

    #[test]
    fn handles_underscore_in_names() {
        let url = "git@ssh.dev.azure.com:v3/Org_Name/Project_Name/Repo_Name";
        let result = parse_azure_git_url(url);
        assert_eq!(
            result,
            Some(AzureRepoComponents {
                org: "Org_Name".to_string(),
                project: "Project_Name".to_string(),
                repo: "Repo_Name".to_string()
            })
        );
    }

    #[test]
    fn handles_dash_in_names() {
        let url = "https://org-name@dev.azure.com/org-name/proj-name/_git/repo-name";
        let result = parse_azure_git_url(url);
        assert_eq!(
            result,
            Some(AzureRepoComponents {
                org: "org-name".to_string(),
                project: "proj-name".to_string(),
                repo: "repo-name".to_string()
            })
        );
    }

    #[test]
    fn mismatched_org_in_https_still_parses() {
        // ORG part in the URL vs path can differ but regex will accept based on the path
        let url = "https://user@dev.azure.com/SomeOrg/SomeProject/_git/SomeRepo";
        let result = parse_azure_git_url(url);
        assert_eq!(
            result,
            Some(AzureRepoComponents {
                org: "SomeOrg".to_string(),
                project: "SomeProject".to_string(),
                repo: "SomeRepo".to_string()
            })
        );
    }

    #[test]
    fn extra_slashes_fail() {
        let url = "https://org@dev.azure.com/org/project/_git/repo/";
        assert_eq!(parse_azure_git_url(url), None);
    }

    #[test]
    fn empty_string_fails() {
        assert_eq!(parse_azure_git_url(""), None);
    }
}
